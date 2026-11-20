use super::*;
use std::future::{poll_fn, Future};
use std::io::Write;
use std::task::Poll;
use std::time::Duration;

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

fn live_sample(
    _: Arc<tokio::sync::Mutex<sysinfo::System>>,
    _: Vec<String>,
    roots: Vec<(String, Option<u32>)>,
) -> Option<SystemProcessSnapshot> {
    Some(SystemProcessSnapshot {
        logical_cpu_count: 1,
        children_map: Arc::new(HashMap::new()),
        processes: Arc::new(
            roots
                .iter()
                .filter_map(|(_, pid)| {
                    pid.map(|pid| {
                        (
                            pid,
                            ProcessSample {
                                cpu_usage: 0.0,
                                memory: 1024,
                                run_time: 60,
                            },
                        )
                    })
                })
                .collect(),
        ),
        sys_refresh: Duration::ZERO,
        #[cfg(windows)]
        session_roots: roots
            .into_iter()
            .filter_map(|(session, pid)| pid.map(|pid| (session, vec![pid])))
            .collect(),
    })
}

async fn registered_live_mock(
    home: &crate::control::test_support::TestWardianHome,
) -> (Arc<AppState>, String, std::path::PathBuf) {
    let state = Arc::new(AppState::new());
    let session = uuid::Uuid::new_v4().to_string();
    let native = crate::commands::agent::planned_provider_session_id("mock").unwrap();
    assert_ne!(session, native);
    let mut agent =
        super::tests::test_active_agent(&session, "mock", "Processing...", Some(424137));
    agent.watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
        session.clone(),
        512,
        4 * 1024 * 1024,
    )));
    let started = "2026-11-07T00:00:00.000Z";
    state
        .conversation_archive
        .begin_live_conversation(&session, started)
        .unwrap();
    agent.watch_state.lock().unwrap().bind_live_chat(started);
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.session_name = session.clone();
        config.folder = home.path().to_string_lossy().into_owned();
        config.fresh_provider_session_id = Some(native.clone());
        config.is_off = false;
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        crate::manager::session_identity::apply_provider_identity("mock", &mut config, &native)
            .unwrap();
        config.clone()
    };
    let dir = home.path().join("agents").join(&session);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mock-transcript.jsonl");
    let init = serde_json::json!({"type":"init", "session_id":native});
    std::fs::write(&path, format!("{init}\n")).unwrap();
    *agent.log_path.lock().unwrap() = Some(path.clone());
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    (state, session, path)
}

async fn append_live_mock_cohort(state: &AppState, session: &str, path: &std::path::Path) {
    // Independent raw Mock user/model/result grammar, including the acceptance
    // fixture's full history, distinct equal-text turns and 60000-byte body.
    let mut turns = (0..96)
        .map(|index| {
            (
                format!("chat-acceptance-history-{index}"),
                format!("Please retain controlled history turn {index}."),
                format!("Controlled assistant history {index} is complete."),
            )
        })
        .collect::<Vec<_>>();
    let mut long_body = "bounded mock body ".repeat(3334);
    long_body.truncate(59999);
    long_body.push('.');
    turns.extend([
        (
            "chat-acceptance-long".into(),
            "Retain the full bounded body.".into(),
            long_body,
        ),
        (
            "chat-acceptance-repeat-a".into(),
            "Complete repeated turn A.".into(),
            "These are two distinct completed assistant turns with equal text.".into(),
        ),
        (
            "chat-acceptance-repeat-b".into(),
            "Complete repeated turn B.".into(),
            "These are two distinct completed assistant turns with equal text.".into(),
        ),
        (
            "chat-acceptance-completed".into(),
            "Complete the independent native timing target.".into(),
            "Native acceptance completed assistant: the amber otter has finished.".into(),
        ),
    ]);
    let (watch, last_modified, native_session) = {
        let agents = state.agents.lock().await;
        let agent = agents.get(session).unwrap();
        let native_session = agent
            .config
            .lock()
            .unwrap()
            .fresh_provider_session_id
            .clone()
            .unwrap();
        (
            agent.watch_state.clone(),
            agent.log_last_modified.clone(),
            native_session,
        )
    };
    let mut watch = watch.lock().unwrap();
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    let incarnation = uuid::Uuid::new_v4().to_string();
    let mut observation = 0;
    for (turn, prompt, body) in turns {
        for mut record in [
            serde_json::json!({"type":"user", "turn_id":turn, "content":prompt}),
            serde_json::json!({"type":"model", "turn_id":turn, "content":body}),
            serde_json::json!({"type":"result", "turn_id":turn, "status":"success"}),
        ] {
            if record["type"] == "model" {
                observation += 1;
                record["wardian_mirror"] = serde_json::json!({
                    "observation_id": format!("{incarnation}:{observation}"),
                    "incarnation": incarnation, "provider_session_id": native_session, "logged": true,
                });
            }
            let line = record.to_string();
            writeln!(file, "{line}").unwrap();
            // The live stdout transcript hook consumes this same raw grammar.
            if let Some(mut message) =
                crate::providers::transcript::extract_transcript_message("mock", &line)
            {
                crate::providers::transcript::bind_mock_watch_message(
                    &mut message,
                    &native_session,
                    &path.to_string_lossy(),
                );
                watch.push_transcript(message);
            }
        }
    }
    file.flush().unwrap();
    *last_modified.lock().unwrap() = Some(std::fs::metadata(path).unwrap().modified().unwrap());
}

async fn dispatch_live_capture(state: &Arc<AppState>, session: &str) -> usize {
    let incarnation = state
        .agents
        .lock()
        .await
        .get(session)
        .unwrap()
        .current_status
        .clone();
    let revision = state.status_revision(session, &incarnation);
    let (_, follow_up) = collect_agent_metrics_with_sampler(state, live_sample).await;
    assert!(follow_up.wake_sessions.is_empty());
    assert!(follow_up.deferred.is_empty());
    assert_eq!(state.status_revision(session, &incarnation), revision);
    let mut workers = Vec::new();
    status::dispatch_capture_follow_up(state, follow_up.background_captures, |claim| {
        let state = state.clone();
        workers.push(tokio::spawn(async move {
            crate::commands::background_capture::run_background_capture(&state, claim).await;
        }));
    })
    .await;
    let admitted = workers.len();
    for worker in workers {
        worker.await.unwrap();
    }
    let agents = state.agents.lock().await;
    let agent = agents.get(session).unwrap();
    assert_eq!(*agent.current_status.lock().unwrap(), "Processing...");
    assert!(!agent.config.lock().unwrap().is_off);
    assert_eq!(agent.process_id, Some(424137));
    admitted
}

async fn queued_live_recent(
    state: &Arc<AppState>,
    session: &str,
    revision: Option<String>,
) -> wardian_core::models::chat::AgentChatPage {
    // App-free host of the normal queue_recent_capture_for_app prelude: keep
    // the actual recent nonce and runtime alive until the blocking job joins.
    let (snapshot, runtime) =
        crate::commands::chat::chat_read_snapshot_for_state(state, session).unwrap();
    if let Some(nonce) = state
        .conversation_archive
        .chat_projection
        .request_recent(&snapshot, &runtime)
        .unwrap()
    {
        let publishing = state.clone();
        let owned_session = session.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            crate::commands::chat::publish_recent_for_state(
                &publishing,
                &owned_session,
                Some(&runtime),
            )
        })
        .await;
        state
            .conversation_archive
            .chat_projection
            .finish_recent_request(&nonce);
        result.unwrap().unwrap();
    }
    crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        state,
        session.into(),
        None,
        revision,
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: None,
            preview: true,
        },
    )
    .await
    .unwrap()
}

async fn assert_live_cohort_handoff(state: &Arc<AppState>, session: &str) {
    let preview = queued_live_recent(state, session, None).await;
    assert_eq!(
        preview.events.len(),
        80,
        "the full default preview cohort must be covered"
    );
    assert!(preview.recent_admission.is_some());
    assert!(preview.conversation_id.is_none());
    assert!(preview.next_before.is_none());
    assert_eq!(
        dispatch_live_capture(state, session).await,
        1,
        "ordinary LIVE observation must admit a worker"
    );
    let ordinary = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        state,
        session.into(),
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    if ordinary.progress != "ready" {
        let diagnostics = std::env::var_os("WARDIAN_HOME")
            .and_then(|root| {
                std::fs::read_to_string(std::path::PathBuf::from(root).join("wardian_debug.log"))
                    .ok()
            })
            .unwrap_or_default();
        for line in diagnostics
            .lines()
            .filter(|line| line.contains("background conversation capture failed"))
        {
            eprintln!("CAPTURE_FAILURE: {line}");
        }
    }
    assert_eq!(
        ordinary.progress, "ready",
        "the real worker must publish canonical history"
    );
    assert_eq!(ordinary.events.len(), 80);
    assert!(
        ordinary.next_before.is_some(),
        "canonical history must exceed one page"
    );
    let canonical = queued_live_recent(state, session, Some(preview.revision)).await;
    assert_eq!(canonical.progress, "ready");
    assert!(
        canonical.recent_admission.is_none(),
        "strict full-cohort coverage must permit canonical handoff"
    );
    assert!(canonical.conversation_id.is_some());
    assert!(canonical.generation.is_some());
    assert!(canonical.source_epoch.is_some());
    assert!(
        canonical.next_before.is_some(),
        "96 historical turns require a real Older cursor"
    );
    assert!(canonical.reset);
    assert_eq!(
        dispatch_live_capture(state, session).await,
        0,
        "unchanged settled source cannot spawn another worker"
    );
    let mut rows = canonical.events;
    let mut cursor = canonical.next_before;
    for _ in 0..8 {
        let Some(before) = cursor.take() else {
            break;
        };
        let older = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
            state,
            session.into(),
            Some(before),
            None,
            None,
            None,
            crate::commands::chat::RecentChatRead::default(),
        )
        .await
        .unwrap();
        assert!(older.older_page);
        rows.extend(older.events);
        cursor = older.next_before;
    }
    assert!(
        cursor.is_none(),
        "the declared fixture exhausts its Older pages"
    );
    assert_eq!(
        rows.iter()
            .filter(
                |row| row.role == Some(wardian_core::models::chat::AgentChatRole::Assistant)
                    && row.text.as_deref()
                        == Some(
                            "These are two distinct completed assistant turns with equal text."
                        )
            )
            .count(),
        2,
        "Watch and provider-log observations must not duplicate completed turn identities"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiet_replaced_source_finishes_owned_projection_work_without_another_append() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_off_source(&home).await;
    *state.agents.lock().await[&session].log_path.lock().unwrap() = Some(path.clone());
    dispatch_ordinary_capture(&state).await;
    let retired = path.with_extension("retired-jsonl");
    std::fs::rename(&path, &retired).unwrap();
    std::fs::copy(&retired, &path).unwrap();
    dispatch_ordinary_capture(&state).await;
    let quiet_request = crate::commands::chat::background_capture_request(&state, &session)
        .await
        .unwrap()
        .unwrap();
    assert!(
        crate::commands::background_capture::admit_background_capture(&state, quiet_request, false)
            .await
            .is_none()
    );
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session)
        .await
        .unwrap();
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let source_before = std::fs::read(&path).unwrap();
    let capture_path = wardian_core::paths::agent_conversations_dir(&session)
        .unwrap()
        .parent()
        .unwrap()
        .join("conversation-capture.json");
    let capture_before = std::fs::read(&capture_path).unwrap();
    state
        .conversation_archive
        .append_delivered_input_with_context(context, "A quiet-source prompt", None)
        .unwrap();
    assert!(state
        .conversation_archive
        .chat_projection
        .has_pending_capture_work(&session));
    let request = crate::commands::chat::background_capture_request(&state, &session)
        .await
        .unwrap()
        .unwrap();
    let claim =
        crate::commands::background_capture::admit_background_capture(&state, request, false)
            .await
            .expect("owned projection work must bypass source-observation coalescing");
    crate::commands::background_capture::run_background_capture(&state, claim).await;
    assert!(!state
        .conversation_archive
        .chat_projection
        .has_pending_capture_work(&session));
    let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: None,
            preview: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(page.progress, "capture_source_replaced");
    assert!(page
        .events
        .iter()
        .any(|event| event.text.as_deref() == Some("A quiet-source prompt")));
    assert_eq!(std::fs::read(&path).unwrap(), source_before);
    assert_eq!(std::fs::read(capture_path).unwrap(), capture_before);
    let quiet_request = crate::commands::chat::background_capture_request(&state, &session)
        .await
        .unwrap()
        .unwrap();
    assert!(
        crate::commands::background_capture::admit_background_capture(&state, quiet_request, false)
            .await
            .is_none(),
        "settled handoff must coalesce again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_collector_drains_audited_archive_adoption_after_quiet_source_replacement() {
    use wardian_core::control::{ArchiveAdoptionNativeIdentity, ArchiveAdoptionRequest};

    const COHORT_ROWS: usize = 128;
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_off_source(&home).await;
    let header_end = std::fs::metadata(&path).unwrap().len();
    let record = |text: &str| {
        format!(
            "{}\n",
            serde_json::json!({
                "type": "event_msg", "timestamp": "2026-10-04T12:00:00Z",
                "payload": { "type": "user_message", "message": text }
            })
        )
    };
    {
        let mut source = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(
            source,
            "{}",
            record("Excluded before the original policy floor")
        )
        .unwrap();
    }
    dispatch_ordinary_capture(&state).await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session)
        .await
        .unwrap();
    let policy_floor = std::fs::metadata(&path).unwrap().len();
    assert!(policy_floor > header_end && header_end > 0);
    assert_eq!(
        capture_state(&state, &snapshot).unknown_before_offset,
        Some(policy_floor)
    );
    {
        let mut source = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(source, "{}", record("Previously captured owned anchor")).unwrap();
    }
    dispatch_ordinary_capture(&state).await;
    let original_receipt = capture_state(&state, &snapshot);
    let start = original_receipt.committed_offset;
    assert!(start > policy_floor);
    assert_eq!(original_receipt.unknown_before_offset, Some(policy_floor));
    assert_eq!(original_receipt.status, "complete");
    assert_eq!(
        original_receipt.continuity_anchor.start + original_receipt.continuity_anchor.len,
        start
    );
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    state
        .conversation_archive
        .append_delivered_input_with_context(
            context.clone(),
            "Existing delivered prompt before adoption",
            None,
        )
        .unwrap();
    let conversation_id = state
        .conversation_archive
        .active_conversation_id(&session)
        .unwrap()
        .unwrap();
    dispatch_ordinary_capture(&state).await;

    // Retain the original inode/file ID; equal bytes and a matching anchor must
    // not make the replacement a natively admitted continuation.
    let original_bytes = std::fs::read(&path).unwrap();
    assert_eq!(original_bytes.len() as u64, start);
    assert_eq!(original_bytes.last(), Some(&b'\n'));
    let retired = path.with_extension("retired-jsonl");
    std::fs::rename(&path, &retired).unwrap();
    std::fs::copy(&retired, &path).unwrap();
    {
        let mut source = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        for index in 0..COHORT_ROWS {
            write!(
                source,
                "{}",
                record(&format!(
                    "Audited adopted cohort {index:03}: {}",
                    "x".repeat(4096)
                ))
            )
            .unwrap();
        }
    }
    let replacement_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        &replacement_bytes[..start as usize],
        original_bytes.as_slice()
    );
    let end = replacement_bytes.len() as u64;
    assert!(end - start > crate::commands::provider_log_acquisition::PROVIDER_LOG_BATCH_BYTES);
    let mut replacement = std::fs::File::open(&path).unwrap();
    let identity =
        crate::commands::provider_log_acquisition::native_file_identity(&replacement).unwrap();
    assert_ne!(identity, original_receipt.native_identity);
    assert!(
        crate::commands::provider_log_acquisition::published_anchor_matches(
            &mut replacement,
            &original_receipt.continuity_anchor,
        )
        .unwrap()
    );
    drop(replacement);
    dispatch_ordinary_capture(&state).await;
    let replaced_receipt = capture_state(&state, &snapshot);
    assert_eq!(
        replaced_receipt.reason.as_deref(),
        Some("provider_log_source_replaced")
    );
    assert_eq!(replaced_receipt.committed_offset, start);
    assert_eq!(replaced_receipt.unknown_before_offset, Some(policy_floor));
    assert_eq!(
        replaced_receipt.native_identity,
        original_receipt.native_identity
    );
    assert!(!state
        .conversation_archive
        .chat_projection
        .has_pending_capture_work(&session));
    assert!(!state
        .conversation_archive
        .has_deferred_chat_summaries(&session));
    let (_, quiet) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    assert!(quiet.wake_sessions.is_empty());
    assert_eq!(quiet.background_captures.len(), 1);
    let unchanged_source = quiet.background_captures[0].source.clone();
    let mut claims = Vec::new();
    status::dispatch_capture_follow_up(&state, quiet.background_captures, |claim| {
        claims.push(claim)
    })
    .await;
    assert!(
        claims.is_empty(),
        "quiet replacement must coalesce before the grant"
    );

    let archive_root = wardian_core::paths::agent_conversations_dir(&session).unwrap();
    let manifest: wardian_core::conversations::ConversationManifest = serde_json::from_slice(
        &std::fs::read(archive_root.join(&conversation_id).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest.status,
        wardian_core::conversations::ConversationStatus::Open
    );
    assert_eq!(
        manifest.effective_logging,
        wardian_core::conversations::ConversationLoggingSetting::Enabled
    );
    let archive_root = archive_root.parent().unwrap();
    let receipt_path = archive_root.join("conversation-capture.json");
    let policy_path = archive_root.join("chat-source-policy.json");
    let receipt_before = std::fs::read(&receipt_path).unwrap();
    let policy_before = std::fs::read(&policy_path).unwrap();
    let policy: serde_json::Value = serde_json::from_slice(&policy_before).unwrap();
    assert_eq!(policy["valid"], false);
    assert_eq!(policy["unknown_before"], policy_floor);
    assert_eq!(policy["blocked_reason"], "provider_log_source_replaced");

    let authorization_id = uuid::Uuid::new_v4().to_string();
    let request = ArchiveAdoptionRequest {
        authorization_id: authorization_id.clone(),
        conversation_id: conversation_id.clone(),
        provider_session: snapshot.resume_session.clone().unwrap(),
        source_path: std::fs::canonicalize(&path)
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
        start_offset: start,
        end_offset: end,
        native_identity: ArchiveAdoptionNativeIdentity {
            platform: identity.platform,
            primary: identity.primary,
            secondary: identity.secondary,
        },
        approval_reference: "recorded-user-approval:ordinary-collector-regression".into(),
        approval_text:
            "Adopt exactly the selected replacement interval into this open conversation.".into(),
    };
    let response = crate::commands::chat::authorize_archive_adoption(
        &state,
        &session,
        request.clone(),
        None,
        "test-operator".into(),
    )
    .await
    .unwrap();
    assert!(response.ok);
    assert_eq!(response.status, "pending");
    assert_eq!(response.agent_id, session);
    assert_eq!(response.conversation_id, conversation_id);
    let audit = wardian_core::conversations::read_jsonl_records::<serde_json::Value>(
        &archive_root.join("archive-adoption-history.jsonl"),
    )
    .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["event"], "archive_adoption_authorized");
    assert_eq!(audit[0]["outcome"], "authorization_only");
    assert_eq!(
        audit[0]["grant"]["request"],
        serde_json::to_value(request).unwrap()
    );
    assert!(state
        .conversation_archive
        .has_pending_adoption(&session)
        .unwrap());
    assert!(!state
        .conversation_archive
        .chat_projection
        .has_pending_capture_work(&session));
    assert_eq!(std::fs::read(&receipt_path).unwrap(), receipt_before);
    assert_eq!(std::fs::read(&policy_path).unwrap(), policy_before);

    let (_, follow_up) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    assert!(follow_up.wake_sessions.is_empty());
    assert_eq!(follow_up.background_captures.len(), 1);
    assert_eq!(follow_up.background_captures[0].source, unchanged_source);
    status::dispatch_capture_follow_up(&state, follow_up.background_captures, |claim| {
        claims.push(claim)
    })
    .await;
    assert_eq!(
        claims.len(),
        1,
        "adoption demand must admit an unchanged source without force"
    );
    let claim = claims.pop().unwrap();
    let work_path = archive_root
        .join("archive-adoptions")
        .join(format!("{authorization_id}.json"));
    let mut worker = Box::pin(crate::commands::background_capture::run_background_capture(
        &state, claim,
    ));
    // Poll the real worker to its first persisted adoption chunk, then pause
    // at its cooperative boundary to append ordinary owned input deterministically.
    poll_fn(|cx| {
        assert!(
            worker.as_mut().poll(cx).is_pending(),
            "the selected interval spans multiple passes"
        );
        let work: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&work_path).unwrap()).unwrap();
        let cursor = work["cursor"].as_u64().unwrap();
        if cursor > start {
            assert!(
                cursor < end,
                "pause while adoption still has selected bytes to import"
            );
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    state
        .conversation_archive
        .append_delivered_input_with_context(
            context.clone(),
            "Normal delivered prompt during archive adoption",
            None,
        )
        .unwrap();
    worker.await;
    let work: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&work_path).unwrap()).unwrap();
    assert_eq!(work["status"], "complete");
    assert_eq!(work["cursor"], end);
    assert!(!state
        .conversation_archive
        .has_pending_adoption(&session)
        .unwrap());
    assert!(!state
        .conversation_archive
        .chat_projection
        .has_pending_capture_work(&session));

    let mut cursor = None;
    let mut rows = Vec::new();
    for page_index in 0..8 {
        let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
            &state,
            session.clone(),
            cursor.take(),
            None,
            None,
            None,
            crate::commands::chat::RecentChatRead::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            page.progress, "capture_source_replaced",
            "complete owned history must retain the invalid native source disposition"
        );
        assert!(
            page.archive_read_ready,
            "initial and Older use the qualified archive reader"
        );
        assert_eq!(
            page.conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        assert!(
            page.source_epoch.is_none(),
            "archive adoption grants no native source epoch"
        );
        assert!(
            page.recent_admission.is_none(),
            "archive adoption grants no native recent admission"
        );
        assert!(
            page.revalidated_ids.is_none(),
            "archive reads cannot acknowledge native renewal"
        );
        assert!(page
            .events
            .iter()
            .all(|event| event.metadata["chat_source_epoch"].is_null()
                && event.metadata["chat_recent_admission"].is_null()));
        if page_index == 0 {
            assert!(
                page.next_before.is_some(),
                "the full cohort requires real Older pages"
            );
            let unchanged = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
                &state,
                session.clone(),
                None,
                Some(page.revision.clone()),
                None,
                None,
                crate::commands::chat::RecentChatRead::default(),
            )
            .await
            .unwrap();
            assert!(unchanged.archive_read_ready);
            assert!(unchanged.unchanged);
            assert!(unchanged.events.is_empty());
            assert_eq!(unchanged.progress, "capture_source_replaced");
            assert_eq!(unchanged.generation, page.generation);
            assert!(unchanged.source_epoch.is_none());
            assert!(unchanged.recent_admission.is_none());
            assert!(unchanged.revalidated_ids.is_none());
            let stale = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
                &state,
                session.clone(),
                Some("source:stale".into()),
                Some(page.revision.clone()),
                None,
                None,
                crate::commands::chat::RecentChatRead::default(),
            )
            .await
            .unwrap();
            assert!(stale.reset);
            assert!(
                !stale.archive_read_ready,
                "a native cursor fallback cannot inherit archive authority"
            );
            assert!(stale.revalidated_ids.is_none());
        } else {
            assert!(page.older_page);
        }
        cursor = page.next_before;
        rows.extend(page.events);
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        cursor.is_none(),
        "the declared cohort exhausts its Older pages"
    );
    let stored: Vec<wardian_core::models::chat::AgentChatEvent> =
        wardian_core::conversations::read_jsonl_records(
            &wardian_core::paths::agent_conversation_dir(&session, &conversation_id)
                .unwrap()
                .join("events.jsonl"),
        )
        .unwrap();
    let adopted_ids: std::collections::HashSet<_> = stored
        .iter()
        .filter(|event| {
            event.metadata["archive_adoption"]["authorization_id"].as_str()
                == Some(authorization_id.as_str())
        })
        .map(|event| event.id.clone())
        .collect();
    assert_eq!(adopted_ids.len(), COHORT_ROWS);
    // The normal read DTO deliberately omits private grant/chunk evidence.
    // Physical observation IDs still bind every public row to its owned cohort.
    let adopted: Vec<_> = rows
        .iter()
        .filter(|event| adopted_ids.contains(&event.id))
        .collect();
    assert_eq!(
        adopted.len(),
        COHORT_ROWS,
        "import the entire approved cohort exactly once"
    );
    let owned = adopted[0];
    let detail_ref = owned.metadata["chat_detail_ref"]
        .as_str()
        .expect("the owned body exceeds the header preview");
    let body = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        None,
        Some(detail_ref.into()),
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert!(body.archive_read_ready);
    assert_eq!(body.progress, "capture_source_replaced");
    assert!(body.source_epoch.is_none());
    assert!(body.recent_admission.is_none());
    assert!(body.revalidated_ids.is_none());
    let detail = body.detail.unwrap();
    assert_eq!(detail.event_id, owned.id);
    assert!(detail.complete);
    assert!(detail.next.is_none());
    assert_eq!(
        detail.text,
        stored
            .iter()
            .find(|event| event.id == owned.id)
            .unwrap()
            .text
            .as_deref()
            .unwrap()
    );
    for index in 0..COHORT_ROWS {
        let prefix = format!("Audited adopted cohort {index:03}:");
        assert_eq!(
            adopted
                .iter()
                .filter(|event| event
                    .text
                    .as_deref()
                    .is_some_and(|text| text.starts_with(&prefix)))
                .count(),
            1,
            "approved record {index} remains visible exactly once"
        );
    }
    for text in [
        "Previously captured owned anchor",
        "Existing delivered prompt before adoption",
        "Normal delivered prompt during archive adoption",
    ] {
        assert_eq!(
            rows.iter()
                .filter(|event| event.text.as_deref() == Some(text))
                .count(),
            1
        );
    }
    assert!(!rows
        .iter()
        .any(|event| event.text.as_deref() == Some("Excluded before the original policy floor")));
    assert_eq!(capture_state(&state, &snapshot), replaced_receipt);
    assert_eq!(std::fs::read(&receipt_path).unwrap(), receipt_before);
    assert_eq!(std::fs::read(&policy_path).unwrap(), policy_before);
    assert!(crate::state::conversation_archive::chat_read::source_policy(&context).is_err());
    assert!(crate::commands::chat_recent_seed::source_scope_with_policy(&snapshot).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), replacement_bytes);
    assert_eq!(std::fs::read(&retired).unwrap(), original_bytes);
    let (_, quiet) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    assert!(quiet.wake_sessions.is_empty());
    assert_eq!(quiet.background_captures.len(), 1);
    assert_eq!(quiet.background_captures[0].source, unchanged_source);
    status::dispatch_capture_follow_up(&state, quiet.background_captures, |claim| {
        claims.push(claim)
    })
    .await;
    assert!(
        claims.is_empty(),
        "completed adoption and projection must coalesce without another append"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_live_first_source_covers_full_recent_cohort() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_live_mock(&home).await;
    append_live_mock_cohort(&state, &session, &path).await;
    assert_live_cohort_handoff(&state, &session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_mock_headers_wait_for_source_renewal_qualification() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_live_mock(&home).await;
    append_live_mock_cohort(&state, &session, &path).await;
    let request = crate::commands::chat::background_capture_request(&state, &session)
        .await
        .unwrap()
        .unwrap();
    for _ in 0..2 {
        crate::commands::chat::archive_background_capture_pass(&state, &request)
            .await
            .unwrap();
    }
    // Canonical rows can finish before the independent bounded source replay.
    for _ in 0..16 {
        if !state
            .conversation_archive
            .chat_projection
            .advance(&session)
            .unwrap()
        {
            break;
        }
    }
    let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert!(!page.events.is_empty());
    assert!(page
        .events
        .iter()
        .any(|event| event.metadata["mock_watch_alias"].is_string()));
    assert_eq!(
        page.progress, "indexing",
        "readable canonical headers do not complete source renewal"
    );
    let unchanged = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        Some(page.revision),
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert_eq!(unchanged.progress, "indexing");
    assert!(unchanged.unchanged);
    assert_eq!(dispatch_live_capture(&state, &session).await, 1);
    let ready = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert_eq!(ready.progress, "ready");
    assert!(ready.next_before.is_some());
    let polled = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        Some(unchanged.revision),
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        polled.progress, "ready",
        "source qualification must reach the polling consumer"
    );
    assert_eq!(
        polled.events.len(),
        80,
        "the ready transition supplies a complete recent window"
    );
    assert!(polled.next_before.is_some());
    let acknowledged = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        Some(polled.revision),
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert_eq!(acknowledged.progress, "ready");
    assert!(acknowledged.unchanged);
    let repeated = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        Some(acknowledged.revision),
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert_eq!(repeated.progress, "ready");
    assert!(
        repeated.unchanged && repeated.events.is_empty(),
        "stable qualification does not emit endless snapshots"
    );

    let old_head = crate::state::conversation_archive::chat_read::head(&session).unwrap();
    let agent_directory = home.path().join("agents").join(&session);
    let objects = agent_directory.join("chat-read-objects");
    let published: serde_json::Value =
        serde_json::from_slice(&std::fs::read(objects.join(old_head.as_ref().unwrap())).unwrap())
            .unwrap();
    assert!(published["source_coverage"].is_object());
    for invalid in ["absent", "admission", "unsettled", "pending"] {
        let mut header = published.clone();
        header.as_object_mut().unwrap().remove("recent_page");
        match invalid {
            "absent" => {
                header.as_object_mut().unwrap().remove("source_coverage");
            }
            "admission" => {
                header["source_coverage"]["admission"] = serde_json::json!("f".repeat(64))
            }
            "unsettled" => {
                header["source_coverage"]["normalizer_settled"] = serde_json::json!(false)
            }
            "pending" => header["progress"] = serde_json::json!("indexing"),
            _ => unreachable!(),
        }
        // Authenticate a well-formed saved Head so this exercises coverage
        // validation, rather than failing its ordinary content-hash check.
        let bytes = serde_json::to_vec(&header).unwrap();
        let reference = crate::commands::chat_recent_seed::hash(&bytes);
        std::fs::write(objects.join(&reference), bytes).unwrap();
        wardian_core::conversations::write_json_atomic(
            &agent_directory.join("chat-read-head.json"),
            &Some(reference),
        )
        .unwrap();
        let withheld = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
            &state,
            session.clone(),
            None,
            None,
            None,
            None,
            crate::commands::chat::RecentChatRead::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            withheld.progress, "indexing",
            "{invalid} coverage cannot grant ready"
        );
    }
    wardian_core::conversations::write_json_atomic(
        &agent_directory.join("chat-read-head.json"),
        &old_head,
    )
    .unwrap();
    append_live_mock_cohort(&state, &session, &path).await;
    let (snapshot, _) =
        crate::commands::chat::chat_read_snapshot_for_state(&state, &session).unwrap();
    let mut pending = true;
    for _ in 0..64 {
        pending =
            crate::state::conversation_archive::chat_read::advance_source_index(&snapshot).unwrap();
        if !pending {
            break;
        }
    }
    assert!(!pending, "the new source interval independently completes");
    assert_eq!(
        crate::state::conversation_archive::chat_read::head(&session).unwrap(),
        old_head
    );
    let stale_canonical = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session,
        None,
        Some(repeated.revision),
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        stale_canonical.progress, "indexing",
        "a ready old canonical Head cannot borrow coverage from a newer complete source interval"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_live_history_progresses_beside_saved_off_capture() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_live_mock(&home).await;
    let saved = super::tests::test_active_agent("saved-capture-competitor", "codex", "Off", None);
    let saved_config = {
        let mut config = saved.config.lock().unwrap();
        config.is_off = true;
        config.folder = home.path().to_string_lossy().into_owned();
        config.resume_session = Some("saved-provider-conversation".into());
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        config.clone()
    };
    let saved_path = home.path().join("saved-competitor.jsonl");
    std::fs::write(&saved_path, "").unwrap();
    append_records(&saved_path, 4096);
    *saved.log_path.lock().unwrap() = Some(saved_path);
    crate::commands::agent::provider_log_tests::register_capture_test_agent(
        &state,
        &saved_config,
        saved,
    )
    .await
    .unwrap();
    append_live_mock_cohort(&state, &session, &path).await;
    let preview = queued_live_recent(&state, &session, None).await;
    assert!(preview.recent_admission.is_some());

    let (_, follow_up) = collect_agent_metrics_with_sampler(&state, live_sample).await;
    assert_eq!(follow_up.background_captures.len(), 2);
    let mut workers = tokio::task::JoinSet::new();
    let (live_finished, live_completion) = tokio::sync::oneshot::channel();
    let mut live_finished = Some(live_finished);
    let mut saved_task = None;
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_secs(30);
    status::dispatch_capture_follow_up(&state, follow_up.background_captures, |claim| {
        let is_live = claim.request.session_id == session;
        let is_saved = claim.request.session_id == saved_config.session_id;
        let finished = if is_live { live_finished.take() } else { None };
        let state = state.clone();
        let worker = workers.spawn(async move {
            crate::commands::background_capture::run_background_capture(&state, claim).await;
            if let Some(finished) = finished {
                let _ = finished.send(());
            }
        });
        if is_saved {
            saved_task = Some(worker);
        }
    })
    .await;
    drop(live_finished);
    let qualifying = state.clone();
    let live_session = session.clone();
    // Retain this task outside the timed observation too: its recent publisher
    // uses spawn_blocking, so dropping its future is not a complete teardown.
    let mut qualification = tokio::spawn(async move {
        live_completion.await.ok()?;
        // Worker exit can also mean a logged capture error. The normal read
        // must qualify canonical history before the original LIVE deadline.
        let canonical =
            queued_live_recent(&qualifying, &live_session, Some(preview.revision)).await;
        let qualified_head =
            crate::state::conversation_archive::chat_read::head(&live_session).unwrap();
        // An already-ready task can beat an expired timeout when polled late.
        // Its own completion time must establish the deadline independently.
        let qualified_at = tokio::time::Instant::now();
        Some((canonical, qualified_head, qualified_at))
    });
    let live = tokio::time::timeout_at(deadline, &mut qualification).await;
    let observation_elapsed = started.elapsed();
    let saved_pending_at_live = saved_task.as_ref().is_some_and(|task| !task.is_finished());
    let mut worker_errors = Vec::new();
    // This is a teardown guard, not the LIVE latency budget. A source-free
    // physical recount has separate bounded-checkpoint coverage.
    let cleanup = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(result) = workers.join_next().await {
            if let Err(error) = result {
                worker_errors.push(error.to_string());
            }
        }
    })
    .await;
    if cleanup.is_err() {
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }
    if live.is_err() {
        // Joining after expiry cannot turn the recorded deadline into a pass.
        // Aborted LIVE capture closes its sender; an active bounded read drains.
        let late_progress = qualification
            .await
            .ok()
            .flatten()
            .map(|(page, _, _)| page.progress);
        eprintln!("LIVE deadline expired; owned qualifier joined with progress {late_progress:?}");
    }
    assert!(
        cleanup.is_ok(),
        "owned saved capture must terminate before teardown"
    );
    assert!(
        worker_errors.is_empty(),
        "capture worker failures: {worker_errors:?}"
    );
    assert!(
        saved_task.is_some(),
        "the saved competitor must actually be dispatched"
    );
    let (canonical, qualified_head, qualified_at) = live
        .expect("LIVE qualification must finish within its original 30-second deadline")
        .expect("the LIVE qualification task must finish successfully")
        .expect("the LIVE worker must finish successfully");
    assert!(
        qualified_at <= deadline,
        "LIVE canonical qualification completed after its original 30-second deadline"
    );
    let qualification_elapsed = qualified_at.duration_since(started);
    assert_eq!(canonical.progress, "ready");
    assert!(canonical.recent_admission.is_none());
    assert!(canonical.conversation_id.is_some());
    assert!(canonical.generation.is_some());
    assert!(canonical.source_epoch.is_some());
    assert!(canonical.next_before.is_some());
    eprintln!("LIVE canonical qualification: {qualification_elapsed:?}; observed at {observation_elapsed:?}; saved capture pending: {saved_pending_at_live}");
    assert_eq!(
        crate::state::conversation_archive::chat_read::head(&session).unwrap(),
        qualified_head,
        "cohort verification must use the publication qualified before the deadline"
    );
    // The real Older cursor pins that immutable publication. Verify all of its
    // rows without charging later Older requests to first-page qualification.
    let mut rows = canonical.events;
    let mut cursor = canonical.next_before;
    for _ in 0..8 {
        let Some(before) = cursor.take() else { break };
        let older = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
            &state,
            session.clone(),
            Some(before),
            None,
            None,
            None,
            crate::commands::chat::RecentChatRead::default(),
        )
        .await
        .unwrap();
        assert!(older.older_page);
        assert_eq!(older.generation, canonical.generation);
        assert_eq!(older.source_epoch, canonical.source_epoch);
        rows.extend(older.events);
        cursor = older.next_before;
    }
    assert!(
        cursor.is_none(),
        "the complete LIVE fixture must exhaust Older"
    );
    let ids = rows
        .iter()
        .map(|row| &row.id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        ids.len(),
        rows.len(),
        "Older must not repeat LIVE identities"
    );
    for role in [
        wardian_core::models::chat::AgentChatRole::User,
        wardian_core::models::chat::AgentChatRole::Assistant,
    ] {
        assert_eq!(
            rows.iter()
                .filter(|row| row.role.as_ref() == Some(&role))
                .count(),
            100
        );
    }
    for index in 0..96 {
        for text in [
            format!("Please retain controlled history turn {index}."),
            format!("Controlled assistant history {index} is complete."),
        ] {
            assert!(
                rows.iter()
                    .any(|row| row.text.as_deref() == Some(text.as_str())),
                "missing qualified LIVE cohort row: {text}"
            );
        }
    }
    assert!(!state
        .conversation_archive
        .chat_projection
        .has_pending_capture_work(&saved_config.session_id));
    let saved_snapshot =
        crate::commands::chat::agent_archive_capture_snapshot(&state, &saved_config.session_id)
            .await
            .unwrap();
    let saved_context =
        crate::commands::chat::conversation_archive_context_from_snapshot(&saved_snapshot);
    let saved_capture = state
        .conversation_archive
        .provider_log_capture_state(
            &saved_config.session_id,
            saved_context.provider_source_key.as_deref().unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        saved_capture.committed_offset,
        std::fs::metadata(&saved_capture.path).unwrap().len()
    );
    eprintln!(
        "saved capture at EOF: status={}, reason={:?}, unknown_before={:?}, archive_head={}",
        saved_capture.status,
        saved_capture.reason,
        saved_capture.unknown_before_offset,
        crate::state::conversation_archive::chat_read::head(&saved_config.session_id)
            .unwrap()
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_live_settled_append_covers_full_recent_cohort() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_live_mock(&home).await;
    // A real cancelled owner makes the original collector service the initial
    // LIVE source without an Off transition or direct archive/index shortcut.
    let request = crate::commands::chat::background_capture_request(&state, &session)
        .await
        .unwrap()
        .unwrap();
    let claim =
        crate::commands::background_capture::admit_background_capture(&state, request, false)
            .await
            .unwrap();
    drop(claim);
    assert_eq!(dispatch_live_capture(&state, &session).await, 1);
    assert_eq!(
        dispatch_live_capture(&state, &session).await,
        0,
        "a settled unchanged source must not admit another capture"
    );
    let before =
        crate::commands::provider_log_acquisition::observe_provider_log_source(&path).unwrap();
    append_live_mock_cohort(&state, &session, &path).await;
    let after =
        crate::commands::provider_log_acquisition::observe_provider_log_source(&path).unwrap();
    assert_eq!(before.native_identity, after.native_identity);
    assert_eq!(before.path, after.path);
    assert!(after.length > before.length);
    assert_live_cohort_handoff(&state, &session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_live_deeper_history_covers_full_recent_cohort() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_live_mock(&home).await;
    // Independent older raw history expands the same real capture fixture;
    // the current Watch still contains the unchanged 96-turn acceptance data.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    for index in 0..512 {
        let turn = format!("independent-older-history-{index}");
        for record in [
            serde_json::json!({"type":"user", "turn_id":turn, "content":format!("Older user {index}")}),
            serde_json::json!({"type":"model", "turn_id":turn, "content":format!("Older assistant {index}")}),
            serde_json::json!({"type":"result", "turn_id":turn, "status":"success"}),
        ] {
            writeln!(file, "{record}").unwrap();
        }
    }
    drop(file);
    append_live_mock_cohort(&state, &session, &path).await;
    let preview = queued_live_recent(&state, &session, None).await;
    assert_eq!(preview.events.len(), 80);
    assert!(preview.recent_admission.is_some());
    assert!(preview.conversation_id.is_none());
    assert!(preview.next_before.is_none());
    assert_eq!(dispatch_live_capture(&state, &session).await, 1);
    let head = crate::state::conversation_archive::chat_read::head(&session)
        .unwrap()
        .unwrap();
    let directory = wardian_core::paths::agent_conversations_dir(&session).unwrap();
    let published: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            directory
                .parent()
                .unwrap()
                .join("chat-read-objects")
                .join(head),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        published["row_count"].as_u64().unwrap() >= 1000,
        "real capture must publish the older cohort"
    );
    let canonical = queued_live_recent(&state, &session, Some(preview.revision)).await;
    assert_eq!(
        canonical.progress, "ready",
        "full80 proof must fit the ordinary per-call budget on the deeper actual head"
    );
    assert!(canonical.recent_admission.is_none());
    assert!(canonical.conversation_id.is_some());
    assert!(canonical.generation.is_some());
    assert!(canonical.source_epoch.is_some());
    assert!(canonical.next_before.is_some());
    assert!(canonical.reset);
}

#[tokio::test]
async fn cold_saved_off_archive_bootstraps_without_new_capture_events() {
    use crate::state::conversation_archive::ConversationArchiveState;
    use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

    for source_available in [false, true] {
        let home = crate::control::test_support::TestWardianHome::new_async().await;
        let (state, session_id, path) = registered_off_source(&home).await;
        dispatch_ordinary_capture(&state).await;
        let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
            .await
            .unwrap();
        let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
        // Build the canonical fixture through its existing writer, then retain
        // only the pre-projection on-disk archive shape in a cold runtime.
        let fixture = ConversationArchiveState::default();
        let events = (0..161)
            .map(|index| AgentChatEvent {
                id: format!("saved-event-{index}"),
                session_id: session_id.clone(),
                provider: "codex".into(),
                kind: AgentChatEventKind::Message,
                role: Some(AgentChatRole::Assistant),
                text: Some(if index == 160 { "saved body ".repeat(3000) } else { format!("saved reply {index}") }),
                title: None, status: None, turn_id: None, source: None,
                command: None, exit_code: None, path: None, language: None,
                created_at: Some("2026-10-04T12:00:00Z".into()),
                sequence: None,
                metadata: serde_json::json!({"provider_log": true, "provider_session_id": "off-provider-conversation", "log_path": path}),
            })
            .collect::<Vec<_>>();
        fixture
            .append_chat_events_with_context(context, &events)
            .unwrap();
        let original = fixture
            .active_conversation_id(&session_id)
            .unwrap()
            .unwrap();
        let conversations = wardian_core::paths::agent_conversations_dir(&session_id).unwrap();
        let directory = conversations.join(&original);
        let saved_paths = [
            "conversation.jsonl",
            "events.jsonl",
            "sources.jsonl",
            "manifest.json",
            "turns.jsonl",
        ]
        .map(|name| directory.join(name));
        let before = saved_paths
            .each_ref()
            .map(|file| std::fs::read(file).unwrap());
        let index_before = std::fs::read(conversations.join("index.jsonl")).unwrap();
        let agent_directory = conversations.parent().unwrap();
        let head_path = agent_directory.join("chat-read-head.json");
        if head_path.exists() {
            std::fs::remove_file(head_path).unwrap();
        }
        let objects = agent_directory.join("chat-read-objects");
        assert!(objects.starts_with(home.path()));
        if objects.exists() {
            std::fs::remove_dir_all(objects).unwrap();
        }
        assert_eq!(
            state
                .conversation_archive
                .active_conversation_id(&session_id)
                .unwrap(),
            None
        );
        if !source_available {
            std::fs::remove_file(&path).unwrap();
        }
        let first = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
            &state,
            session_id.clone(),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            first.events.len() <= 80,
            "first read is bounded and may be provisional"
        );
        crate::commands::background_capture::capture_background_for_state(&state, &session_id)
            .await
            .unwrap();
        let page = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
            &state,
            session_id.clone(),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(page.conversation_id.as_deref(), Some(original.as_str()), "normal background owner must bind saved archive even when source_available={source_available}");
        assert_eq!(page.events.len(), 80);
        let older = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
            &state,
            session_id.clone(),
            page.next_before,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            !older.events.is_empty(),
            "older saved history remains reachable"
        );
        let detail_ref = page
            .events
            .iter()
            .find(|event| event.id == "saved-event-160")
            .unwrap()
            .metadata["chat_detail_ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let detail = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
            &state,
            session_id.clone(),
            None,
            None,
            Some(detail_ref),
            None,
        )
        .await
        .unwrap();
        assert!(
            detail.detail.is_some(),
            "saved body has a bounded detail reader"
        );
        for (file, bytes) in saved_paths.iter().zip(before) {
            assert_eq!(
                std::fs::read(file).unwrap(),
                bytes,
                "bootstrap must not rewrite saved archive"
            );
        }
        assert_eq!(
            std::fs::read(conversations.join("index.jsonl")).unwrap(),
            index_before
        );
    }
}

async fn register_cold_scope_agent(state: &AppState, config: &wardian_core::models::AgentConfig) {
    let active = super::tests::test_active_agent(&config.session_id, &config.provider, "Off", None);
    *active.config.lock().unwrap() = config.clone();
    crate::commands::agent::provider_log_tests::register_capture_test_agent(state, config, active)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_publisher_recent_headers_precede_older_archive_gate() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    let session_id = "watch-recent-before-older";
    let mut agent = super::tests::test_active_agent(session_id, "mock", "Off", None);
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.is_off = true;
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        config.clone()
    };
    agent.watch_state = Arc::new(std::sync::Mutex::new(crate::state::AgentWatchState::new(
        session_id.into(),
        512,
        4 * 1024 * 1024,
    )));
    let watch = agent.watch_state.clone();
    let message = |turn: String, text: String| wardian_core::control::WatchTranscriptMessage {
        role: "assistant".into(),
        text,
        provider: "mock".into(),
        turn_id: Some(turn),
        source: Some("model".into()),
        provider_provenance: None,
    };
    {
        let mut watch = watch.lock().unwrap();
        for index in 0..256 {
            watch.push_transcript(message(
                format!("old-{index}"),
                format!("Earlier turn {index}: {}", "older body ".repeat(1024)),
            ));
        }
    }
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    // Watch-only Mock has no JSONL telemetry source. Exercise the real common
    // archive publisher used by the existing Watch projection test, rather
    // than pretending that log-based telemetry admitted a capture request.
    let first = crate::commands::chat::archive_agent_chat_events_for_state(&state, session_id)
        .await
        .expect("initial real Watch publication");
    assert!(first
        .events
        .iter()
        .any(|event| event.turn_id.as_deref() == Some("old-255")));
    assert_eq!(
        first
            .events
            .iter()
            .filter(|event| event.kind == wardian_core::models::chat::AgentChatEventKind::Message)
            .count(),
        256,
        "fixture publishes the declared older cohort, not a truncated Watch tail"
    );

    let gate = state
        .conversation_archive
        .test_agent_archive_gate(session_id)
        .unwrap();
    let held_gate = gate.clone();
    let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let holder = std::thread::spawn(move || {
        let _guard = held_gate.lock().unwrap();
        locked_tx.send(()).unwrap();
        release_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("test releases owned gate");
    });
    locked_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let holders_before_capture = Arc::strong_count(&gate);
    let latest_text = "Latest owned Watch header before old archive IO";
    watch
        .lock()
        .unwrap()
        .push_transcript(message("latest-watch-turn".into(), latest_text.into()));
    let capture_state = state.clone();
    let capture = tokio::spawn(async move {
        crate::commands::chat::archive_agent_chat_events_for_state(&capture_state, session_id).await
    });
    let recent = tokio::time::timeout(Duration::from_secs(2), async {
        // The extra real gate reference proves that the producer reached the
        // old archive operation. No guessed sleep or fabricated ready head.
        while Arc::strong_count(&gate) <= holders_before_capture {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        crate::commands::chat::load_agent_chat_page_with_recent_for_state(
            &state,
            session_id.into(),
            None,
            None,
            None,
            None,
            crate::commands::chat::RecentChatRead {
                limit: Some(8),
                preview: true,
            },
        )
        .await
    })
    .await;
    // Always release and join owned work before making the behavior assertion.
    release_tx.send(()).unwrap();
    let captured = tokio::time::timeout(Duration::from_secs(10), capture).await;
    let released = holder.join();
    assert!(released.is_ok(), "owned archive gate thread joined");
    assert!(
        captured.unwrap().unwrap().is_ok(),
        "real Watch publisher completed after gate release"
    );
    let recent = recent
        .expect("ordinary recent read remains bounded")
        .unwrap();
    assert!(
        recent
            .events
            .iter()
            .any(|event| event.text.as_deref() == Some(latest_text)
                && event.turn_id.as_deref() == Some("latest-watch-turn")),
        "actual newly observed Watch header must precede old archive completion"
    );
    assert!((1..=8).contains(&recent.events.len()));
    assert!(serde_json::to_vec(&recent).unwrap().len() <= 256 * 1024);
    assert!(
        recent.next_before.is_none() && recent.detail.is_none() && recent.revalidated_ids.is_none()
    );
    assert!(
        recent
            .events
            .iter()
            .all(|event| event.session_id == session_id
                && event.provider == "mock"
                && event.metadata["chat_source_ref"].is_null()),
        "Watch is not a forged native-frame source"
    );
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_antigravity_observed_database_prelude_and_normal_canonical_handoff() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let native_session = "00000000-0000-4000-8000-000000000274";
    let folder = home.path().join("conversations");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(format!("{native_session}.db"));
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE steps(idx INTEGER PRIMARY KEY,step_type INTEGER,step_payload BLOB)",
        )
        .unwrap();
    let fixture: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../providers/antigravity/fixtures/chat-tools-1.1.27.json"
    ))
    .unwrap();
    for row in fixture {
        let bytes = row["step_payload_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        connection
            .execute(
                "INSERT INTO steps VALUES (?1,?2,?3)",
                rusqlite::params![
                    row["step_index"].as_i64().unwrap(),
                    row["step_type"].as_i64().unwrap(),
                    bytes
                ],
            )
            .unwrap();
    }
    let state = Arc::new(AppState::new());
    let session = "recent-antigravity-native";
    let agent = super::tests::test_active_agent(session, "antigravity", "Off", None);
    *agent.log_path.lock().unwrap() = Some(path);
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.is_off = true;
        config.resume_session = Some(native_session.into());
        config.folder = home.path().to_string_lossy().into_owned();
        config.reset_provider_config_for_provider();
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        config.clone()
    };
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    crate::commands::chat::publish_recent_for_state(&state, session, None).unwrap();
    let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.into(),
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(2),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert!(
        page.recent_admission.is_some()
            && page.next_before.is_none()
            && page.revalidated_ids.is_none()
    );
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.metadata["step_index"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [4, 6]
    );
    assert!(page
        .events
        .iter()
        .all(|event| event.sequence.is_none() && event.id.starts_with("preview:")));
    assert!(state
        .conversation_archive
        .chat_events_for_agent(session)
        .unwrap()
        .is_empty());
    crate::commands::chat::archive_agent_chat_events_until_stable_for_state(&state, session)
        .await
        .unwrap();
    crate::commands::chat::publish_recent_for_state(&state, session, None).unwrap();
    let canonical = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.into(),
        None,
        Some(page.revision),
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(2),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert!(canonical.recent_admission.is_none() && canonical.reset && !canonical.unchanged);
    assert!(canonical
        .events
        .iter()
        .any(|event| event.turn_id.as_deref() == Some("6")
            && event.text.as_ref().is_some_and(|text| !text.is_empty())));
    assert!(canonical
        .events
        .iter()
        .all(|event| !event.id.starts_with("preview:")));
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_opencode_normal_selection_publishes_native_parts_before_archive_and_hands_off() {
    use tauri::Manager;
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    struct RestoreXdg(Option<std::ffi::OsString>);
    impl Drop for RestoreXdg {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("XDG_DATA_HOME", value),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
        }
    }
    let _xdg = RestoreXdg(std::env::var_os("XDG_DATA_HOME"));
    let provider_root = home.path().join("provider-data");
    std::env::set_var("XDG_DATA_HOME", &provider_root);
    let folder = provider_root.join("opencode");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("opencode.db");
    let log_dir = folder.join("log");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_path = log_dir.join("opencode.log");
    std::fs::write(&log_path, b"").unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT); CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT); CREATE INDEX part_recent ON part(session_id,time_created,id);").unwrap();
    connection
        .execute(
            "INSERT INTO message VALUES ('message','ses_recent',1000,'{\"role\":\"assistant\"}')",
            [],
        )
        .unwrap();
    for index in 0..3 {
        connection
            .execute(
                "INSERT INTO part VALUES (?1,'message','ses_recent',?2,?3)",
                rusqlite::params![
                    format!("part-{index}"),
                    1001 + index,
                    format!("{{\"type\":\"text\",\"text\":\"native-body-{index}\"}}")
                ],
            )
            .unwrap();
    }
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let session = "recent-native-selection";
    let agent = super::tests::test_active_agent(session, "opencode", "Off", None);
    *agent.log_path.lock().unwrap() = Some(log_path);
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.is_off = true;
        config.resume_session = Some("ses_recent".into());
        config.folder = home.path().to_string_lossy().into_owned();
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        config.clone()
    };
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    let gate = state
        .conversation_archive
        .test_agent_archive_gate(session)
        .unwrap();
    let (held_tx, held_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let holder = std::thread::spawn(move || {
        let _guard = gate.lock().unwrap();
        held_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    held_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    crate::commands::chat::queue_recent_capture_for_app(&state, app.handle(), session).unwrap();
    let selected = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
                &state,
                session.into(),
                None,
                None,
                None,
                None,
                crate::commands::chat::RecentChatRead {
                    limit: Some(2),
                    preview: true,
                },
            )
            .await
            .unwrap();
            if page.recent_admission.is_some() {
                break page;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    let page = selected.unwrap();
    let joined = tokio::time::timeout(Duration::from_secs(2), async {
        while state
            .conversation_archive
            .chat_projection
            .recent_pending_for_test()
            != 0
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    joined.unwrap();
    assert_eq!(
        page.events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>(),
        ["native-body-1", "native-body-2"]
    );
    assert!(page.events.iter().all(|event| event.sequence.is_none()
        && event.id.starts_with("preview:")
        && event.metadata["opencode_session_id"] == "ses_recent"
        && event.metadata["part_id"].is_string()));
    assert!(page.next_before.is_none() && page.revalidated_ids.is_none());
    assert!(state
        .conversation_archive
        .chat_events_for_agent(session)
        .unwrap()
        .is_empty());
    crate::commands::chat::archive_agent_chat_events_until_stable_for_state(&state, session)
        .await
        .unwrap();
    // Normal policy observation can establish the byte-log identity during
    // capture. The next ordinary selection refresh binds that actual scope.
    crate::commands::chat::queue_recent_capture_for_app(&state, app.handle(), session).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while state
            .conversation_archive
            .chat_projection
            .recent_pending_for_test()
            != 0
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let canonical = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.into(),
        None,
        Some(page.revision.clone()),
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(2),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert!(
        canonical.recent_admission.is_none() && canonical.reset && !canonical.unchanged,
        "published normal original IDs must cover every private native key/version before handoff"
    );
    assert!(
        canonical
            .events
            .iter()
            .all(|event| !event.id.starts_with("preview:"))
            && canonical.events.iter().any(|event| event
                .text
                .as_ref()
                .is_some_and(|text| text.contains("native-body-2"))),
        "handoff must serve the actual latest canonical content, not an empty reset"
    );
    let persisted = state
        .conversation_archive
        .chat_events_for_agent(session)
        .unwrap();
    assert_eq!(
        persisted
            .iter()
            .filter(|event| event.source.as_deref() == Some("opencode_db"))
            .count(),
        3
    );
    assert!(persisted
        .iter()
        .all(|event| !event.id.starts_with("preview:")));
    connection.execute("UPDATE part SET data='{\"type\":\"text\",\"text\":\"edited native-body-2\"}' WHERE id='part-2'", []).unwrap();
    crate::commands::chat::publish_recent_for_state(&state, session, None).unwrap();
    let edited = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.into(),
        None,
        Some(canonical.revision.clone()),
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(2),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert!(
        edited.recent_admission.is_some(),
        "old canonical version cannot cover edited part"
    );
    assert!(
        !edited.events.is_empty(),
        "an edited native version must provide its actual headers"
    );
    assert_eq!(
        edited.events.last().unwrap().text.as_deref(),
        Some("edited native-body-2")
    );
    crate::commands::chat::archive_agent_chat_events_until_stable_for_state(&state, session)
        .await
        .unwrap();
    crate::commands::chat::publish_recent_for_state(&state, session, None).unwrap();
    // A source-proved edit must reach real canonical publication while
    // retaining the original observation and narrative identity.
    let after_capture = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.into(),
        None,
        Some(edited.revision.clone()),
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(2),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert!(
        after_capture.recent_admission.is_none() && after_capture.reset,
        "an actual refreshed canonical body/version must retire the preview"
    );
    assert_eq!(
        after_capture.events.last().unwrap().text.as_deref(),
        Some("edited native-body-2")
    );
    let after_persisted = state
        .conversation_archive
        .chat_events_for_agent(session)
        .unwrap();
    let native_rows: Vec<_> = after_persisted
        .iter()
        .filter(|event| event.source.as_deref() == Some("opencode_db"))
        .collect();
    assert_eq!(
        native_rows.len(),
        3,
        "observing an edited part cannot append duplicate preview rows to history"
    );
    assert_eq!(
        native_rows
            .iter()
            .map(|event| &event.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    assert_eq!(
        native_rows
            .iter()
            .map(|event| &event.id)
            .collect::<std::collections::HashSet<_>>(),
        persisted
            .iter()
            .filter(|event| event.source.as_deref() == Some("opencode_db"))
            .map(|event| &event.id)
            .collect::<std::collections::HashSet<_>>()
    );
    assert_eq!(
        native_rows
            .iter()
            .find(|event| event.metadata["part_id"] == "part-2")
            .unwrap()
            .text
            .as_deref(),
        Some("edited native-body-2"),
        "source-proved refresh must persist the new body with the original identity"
    );
    let older = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        session.into(),
        Some(
            after_capture
                .next_before
                .clone()
                .expect("three canonical parts exceed the recent limit of two"),
        ),
        Some(after_capture.revision.clone()),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(older.events.len(), 1);
    let original_older = native_rows
        .iter()
        .find(|event| event.metadata["part_id"] == "part-0")
        .unwrap();
    assert_eq!(older.events[0].id, original_older.id);
    assert_eq!(
        older.events[0].metadata["chat_recent_native_key"],
        original_older.metadata["chat_recent_native_key"]
    );
    assert_eq!(
        older.events[0].metadata["chat_recent_native_version"],
        original_older.metadata["chat_recent_native_version"]
    );
    assert_eq!(older.conversation_id, after_capture.conversation_id);
    assert_eq!(older.generation, after_capture.generation);
    assert!(older.recent_admission.is_none());
    assert!(older.next_before.is_none());
    assert!(persisted.iter().any(|event| event.id == older.events[0].id));

    let large_body = format!("{}.", "source-proved materialized body\n".repeat(1200));
    assert_eq!(large_body.trim(), large_body);
    connection
        .execute(
            "UPDATE part SET data=?1 WHERE id='part-2'",
            [serde_json::json!({"type":"text","text":large_body}).to_string()],
        )
        .unwrap();
    crate::commands::chat::archive_agent_chat_events_until_stable_for_state(&state, session)
        .await
        .unwrap();
    let full = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        session.into(),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let original_large = native_rows
        .iter()
        .find(|event| event.metadata["part_id"] == "part-2")
        .unwrap();
    let large = full
        .events
        .iter()
        .find(|event| event.id == original_large.id)
        .unwrap();
    assert!(large.metadata["chat_detail_ref"].is_string());
    assert_eq!(
        missing_body_details(&state, session, large, true).await,
        large_body
    );
    assert_eq!(
        large.id,
        native_rows
            .iter()
            .find(|event| event.metadata["part_id"] == "part-2")
            .unwrap()
            .id
    );
    // This database adapter has no framed SourceIndex source ID. A canonical
    // body/head alone must not manufacture file-source renewal authority.
    assert!(large.metadata["chat_source_ref"].is_null());
    assert!(
        crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
            &state,
            session.into(),
            None,
            Some(full.revision.clone()),
            None,
            Some(vec![large.id.clone()]),
        )
        .await
        .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_live_watch_survives_disabled_durable_capture_without_history() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = AppState::new();
    let session = "recent-live-disabled";
    let agent = super::tests::test_active_agent(session, "mock", "Off", None);
    let started = "2026-11-07T00:00:00.000Z";
    state
        .conversation_archive
        .begin_live_conversation(session, started)
        .unwrap();
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.is_off = true;
        config.folder = home.path().to_string_lossy().into_owned();
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Disabled;
        config.clone()
    };
    {
        let mut watch = agent.watch_state.lock().unwrap();
        watch.push_transcript(
            crate::providers::transcript::extract_transcript_message(
                "mock",
                r#"{"type":"model","content":"pre-boundary old row","turn_id":"old"}"#,
            )
            .unwrap(),
        );
        watch.bind_live_chat(started);
        watch.push_transcript(
            crate::providers::transcript::extract_transcript_message(
                "mock",
                r#"{"type":"model","content":"current owned live row","turn_id":"new"}"#,
            )
            .unwrap(),
        );
        let mut unowned_db = crate::providers::transcript::extract_transcript_message(
            "mock",
            r#"{"type":"model","content":"must not resurrect DB","turn_id":"db"}"#,
        )
        .unwrap();
        unowned_db.source = Some("opencode_db".into());
        watch.push_transcript(unowned_db);
    }
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    crate::commands::chat::archive_agent_chat_events_for_state(&state, session)
        .await
        .unwrap();
    crate::commands::chat::publish_recent_for_state(&state, session, None).unwrap();
    let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.into(),
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(8),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        page.events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>(),
        ["current owned live row"]
    );
    assert!(page.recent_admission.is_some() && page.revalidated_ids.is_none());
    assert!(
        state
            .conversation_archive
            .chat_events_for_agent(session)
            .unwrap()
            .is_empty(),
        "live preview never creates durable history"
    );
    state
        .conversation_archive
        .retire_live_conversation(session)
        .unwrap();
    assert!(crate::commands::chat::publish_recent_for_state(&state, session, None).is_err());
    let retired = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.into(),
        None,
        Some(page.revision),
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(8),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert!(retired.reset && retired.events.is_empty() && retired.recent_admission.is_none());
    {
        let agents = state.agents.lock().await;
        let agent = &agents[session];
        agent.config.lock().unwrap().resume_session = Some("external-old-session".into());
        agent
            .watch_state
            .lock()
            .unwrap()
            .bind_live_chat("2026-11-07T00:00:03.000Z");
    }
    state
        .conversation_archive
        .begin_live_conversation(session, "2026-11-07T00:00:03.000Z")
        .unwrap();
    assert!(crate::commands::chat::publish_recent_for_state(&state, session, None).is_err(),
        "a newly created Watch cannot make replayed external-session history eligible for LIVE override");
}

async fn fresh_bound_codex_live_page(discover_log: bool) {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = AppState::new();
    let session = "recent-fresh-bound-codex";
    let native = "00000000-0000-4000-8000-000000007009";
    let started = "2026-11-07T00:00:00.000Z";
    let agent = super::tests::test_active_agent(session, "codex", "Off", None);
    state
        .conversation_archive
        .begin_live_conversation(session, started)
        .unwrap();
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.is_off = true;
        config.folder = home.path().to_string_lossy().into_owned();
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Disabled;
        crate::manager::session_identity::apply_provider_identity("codex", &mut config, native)
            .unwrap();
        assert_eq!(config.resume_session.as_deref(), Some(native));
        assert_eq!(config.fresh_provider_session_id.as_deref(), Some(native));
        config.clone()
    };
    let frame = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"fresh current stdout answer"}]},"turn_id":"current-turn"}"#;
    {
        let mut watch = agent.watch_state.lock().unwrap();
        watch.bind_live_chat(started);
        watch.push_transcript(
            crate::providers::transcript::extract_transcript_message("codex", frame).unwrap(),
        );
        watch.push_file_transcript(
            crate::providers::transcript::extract_transcript_message(
                "codex",
                &frame.replace(
                    "fresh current stdout answer",
                    "file row cannot borrow current live ownership",
                ),
            )
            .unwrap(),
        );
    }
    if discover_log {
        let path = home.path().join(format!("rollout-{native}.jsonl"));
        let header =
            serde_json::json!({"type":"session_meta","payload":{"id":native,"cwd":home.path()}});
        std::fs::write(&path, format!("{header}\n{frame}\n")).unwrap();
        *agent.log_path.lock().unwrap() = Some(path);
    }
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    crate::commands::chat::archive_agent_chat_events_for_state(&state, session)
        .await
        .unwrap();
    crate::commands::chat::publish_recent_for_state(&state, session, None).unwrap();
    let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.into(),
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(8),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        page.events
            .iter()
            .filter_map(|event| event.text.as_deref())
            .collect::<Vec<_>>(),
        ["fresh current stdout answer"],
        "fresh binding/log discovery cannot retire the same live Watch"
    );
    assert!(page.recent_admission.is_some() && page.revalidated_ids.is_none());
    assert!(
        state
            .conversation_archive
            .chat_events_for_agent(session)
            .unwrap()
            .is_empty(),
        "disabled live admission must not persist history"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_live_watch_after_fresh_codex_binding_without_log() {
    fresh_bound_codex_live_page(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_live_watch_after_fresh_codex_binding_with_owned_log() {
    fresh_bound_codex_live_page(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_all_provider_watch_extractors_precede_first_archive_io() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let fixtures = [
        (
            "codex",
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Codex answer"}]},"turn_id":"turn-1"}"#,
        ),
        (
            "claude",
            r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"Claude answer"}]}}"#,
        ),
        (
            "gemini",
            r#"{"id":"gem-msg-1","type":"model","content":"Gemini answer","tokens":{"input":10,"output":2,"total":12}}"#,
        ),
        (
            "antigravity",
            r#"{"step_index":2,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-05-20T09:21:54Z","content":"Antigravity answer"}"#,
        ),
        (
            "opencode",
            r#"{"type":"text","sessionID":"ses_test","part":{"type":"text","text":"OpenCode answer"}}"#,
        ),
        (
            "pi",
            r#"{"type":"message","id":"pi-entry","message":{"role":"assistant","content":[{"type":"text","text":"Pi answer"}],"stopReason":"stop"}}"#,
        ),
        (
            "mock",
            r#"{"type":"model","content":"Mock answer","turn_id":"turn-7"}"#,
        ),
    ];
    for (provider, frame) in fixtures {
        let state = Arc::new(AppState::new());
        let session_id = format!("recent-watch-{provider}");
        let agent = super::tests::test_active_agent(&session_id, provider, "Off", None);
        let config = {
            let mut config = agent.config.lock().unwrap();
            config.is_off = true;
            config.folder = home.path().to_string_lossy().into_owned();
            config.reset_provider_config_for_provider();
            config.conversation_logging =
                wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
            config.clone()
        };
        let actual = crate::providers::transcript::extract_transcript_message(provider, frame)
            .unwrap_or_else(|| panic!("{provider} production extractor admits its actual format"));
        let expected_text = actual.text.clone();
        let expected_turn = actual.turn_id.clone();
        agent.watch_state.lock().unwrap().push_transcript(actual);
        crate::commands::agent::provider_log_tests::register_capture_test_agent(
            &state, &config, agent,
        )
        .await
        .unwrap();
        let (snapshot, _) =
            crate::commands::chat::chat_read_snapshot_for_state(&state, &session_id).unwrap();
        assert!(
            snapshot.log_path.is_none(),
            "{provider}: genuine Watch-only fixture, no foreign vendor source"
        );
        let gate = state
            .conversation_archive
            .test_agent_archive_gate(&session_id)
            .unwrap();
        let held_gate = gate.clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let holder = std::thread::spawn(move || {
            let _guard = held_gate.lock().unwrap();
            locked_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        locked_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let references = Arc::strong_count(&gate);
        let publishing = state.clone();
        let capture_session = session_id.clone();
        let capture = tokio::spawn(async move {
            crate::commands::chat::archive_agent_chat_events_for_state(
                &publishing,
                &capture_session,
            )
            .await
        });
        let page = tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&gate) <= references {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            crate::commands::chat::load_agent_chat_page_with_recent_for_state(
                &state,
                session_id.clone(),
                None,
                None,
                None,
                None,
                crate::commands::chat::RecentChatRead {
                    limit: Some(8),
                    preview: true,
                },
            )
            .await
        })
        .await;
        release_tx.send(()).unwrap();
        let captured = tokio::time::timeout(Duration::from_secs(10), capture).await;
        holder.join().unwrap();
        assert!(
            captured.unwrap().unwrap().is_ok(),
            "{provider} common publisher joined"
        );
        let page = page
            .unwrap_or_else(|_| panic!("{provider}: bounded ordinary read"))
            .unwrap();
        assert_eq!(
            page.events.len(),
            1,
            "{provider}: exact declared current message"
        );
        assert_eq!(page.events[0].provider, provider);
        assert_eq!(page.events[0].text.as_deref(), Some(expected_text.as_str()));
        assert_eq!(page.events[0].turn_id, expected_turn);
        assert!(
            page.recent_admission.is_some()
                && page.generation.is_none()
                && page.source_epoch.is_none()
                && page.conversation_id.is_none()
        );
        assert!(
            page.revalidated_ids.is_none() && page.aliases.is_empty() && page.next_before.is_none()
        );
        eprintln!("WATCH_FORMAT_PUBLICATION {provider} PASS; native-file/DB/paid-provider acceptance UNRUN");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_watch_prelude_does_not_wait_for_older_global_capture_work() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    let session = "recent-before-global-work";
    let agent = super::tests::test_active_agent(session, "mock", "Off", None);
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.is_off = true;
        config.folder = home.path().to_string_lossy().into_owned();
        config.conversation_logging =
            wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        config.clone()
    };
    agent.watch_state.lock().unwrap().push_transcript(
        wardian_core::control::WatchTranscriptMessage {
            role: "assistant".into(),
            text: "Current header before global work".into(),
            provider: "mock".into(),
            turn_id: Some("current-turn".into()),
            source: Some("model".into()),
            provider_provenance: None,
        },
    );
    crate::commands::agent::provider_log_tests::register_capture_test_agent(&state, &config, agent)
        .await
        .unwrap();
    let global_work = state.conversation_capture_policy_lock.lock().await;
    let mut capture = Box::pin(crate::commands::chat::archive_agent_chat_events_for_state(
        &state, session,
    ));
    let started = std::time::Instant::now();
    let read = tokio::time::timeout(Duration::from_secs(1), async {
        // Drive the real prelude to its first gate wait before a competing
        // reader can make its one best-effort snapshot miss. This work remains
        // inside the original deadline, while old global work stays held.
        poll_fn(|cx| {
            assert!(capture.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        loop {
            let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
                &state,
                session.into(),
                None,
                None,
                None,
                None,
                crate::commands::chat::RecentChatRead {
                    limit: Some(8),
                    preview: true,
                },
            )
            .await
            .unwrap();
            if page.recent_admission.is_some() {
                break (page, started.elapsed());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    drop(global_work);
    let capture = tokio::time::timeout(Duration::from_secs(10), capture).await;
    assert!(
        capture.unwrap().is_ok(),
        "owned full capture joined after releasing old work"
    );
    let (page, elapsed) =
        read.expect("bounded header publication cannot wait behind another old capture");
    assert!(elapsed < Duration::from_secs(1));
    assert_eq!(
        page.events[0].text.as_deref(),
        Some("Current header before global work")
    );
}

async fn missing_body_fixture(
    state: &AppState,
    home: &crate::control::test_support::TestWardianHome,
    body_index: usize,
) -> (
    wardian_core::models::AgentConfig,
    String,
    std::path::PathBuf,
    String,
    Vec<(std::path::PathBuf, Vec<u8>)>,
) {
    use wardian_core::models::{AgentConfig, ProviderConfig};
    let config = AgentConfig {
        session_id: "missing-body-agent".into(),
        session_name: "Missing body".into(),
        provider: "codex".into(),
        is_off: true,
        folder: home.path().to_string_lossy().into_owned(),
        resume_session: Some("missing-body-provider".into()),
        provider_config: ProviderConfig::Codex(Default::default()),
        conversation_logging: wardian_core::conversations::AgentConversationLoggingSetting::Enabled,
        ..Default::default()
    };
    register_cold_scope_agent(state, &config).await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(state, &config.session_id)
        .await
        .unwrap();
    assert!(
        snapshot.log_path.is_none(),
        "saved fixture must not bind a vendor source"
    );
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let body = "saved artifact body\n".repeat(7000);
    let events = (0..161)
        .map(|index| {
            serde_json::from_value(serde_json::json!({
        "id": format!("missing-body-{index}"), "session_id": config.session_id,
        "provider": "codex", "kind": if index == body_index { "tool_result" } else { "message" },
        "role": if index == body_index { "tool" } else { "assistant" },
        "text": if index == body_index { body.clone() } else { format!("saved reply {index}") },
        "metadata": { "provider_log": true }
    })).unwrap()
        })
        .collect::<Vec<wardian_core::models::chat::AgentChatEvent>>();
    let fixture = crate::state::conversation_archive::ConversationArchiveState::default();
    fixture
        .append_chat_events_with_context(context, &events)
        .unwrap();
    let original = fixture
        .active_conversation_id(&config.session_id)
        .unwrap()
        .unwrap();
    let conversations = wardian_core::paths::agent_conversations_dir(&config.session_id).unwrap();
    let directory = conversations.join(&original);
    let saved = [
        "conversation.jsonl",
        "events.jsonl",
        "sources.jsonl",
        "manifest.json",
        "turns.jsonl",
    ]
    .map(|name| directory.join(name))
    .into_iter()
    .chain([conversations.join("index.jsonl")])
    .map(|path| {
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    })
    .collect();
    let stored = std::fs::read_to_string(directory.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<wardian_core::models::chat::AgentChatEvent>(line).unwrap()
        })
        .find(|event| event.id == format!("missing-body-{body_index}"))
        .unwrap();
    let reference = stored.metadata["text_artifact_refs"][0].as_str().unwrap();
    assert_eq!(std::path::Path::new(reference).components().count(), 1);
    let artifact = directory.join("artifacts").join(reference);
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), body);
    let agent_dir = conversations.parent().unwrap();
    for name in ["chat-read-head.json", "chat-read-objects"] {
        let path = agent_dir.join(name);
        if path.exists() {
            let resolved = path.canonicalize().unwrap();
            assert!(
                resolved.starts_with(home.path().canonicalize().unwrap()),
                "test cleanup must stay in its owned home"
            );
            if resolved.is_dir() {
                std::fs::remove_dir_all(resolved).unwrap();
            } else {
                std::fs::remove_file(resolved).unwrap();
            }
        }
    }
    (config, original, artifact, body, saved)
}

async fn missing_body_one_pass(state: &AppState, session_id: &str) {
    let request = crate::commands::chat::background_capture_request(state, session_id)
        .await
        .unwrap()
        .unwrap();
    let claim =
        crate::commands::background_capture::admit_background_capture(state, request, false)
            .await
            .unwrap();
    let result =
        crate::commands::chat::archive_background_capture_pass(state, &claim.request).await;
    drop(claim);
    assert!(
        result.is_ok(),
        "normal owner must advance headers despite a missing individual body: {result:?}"
    );
}

async fn missing_body_resume(state: &AppState, session_id: &str) {
    // Observe finite checkpoint progress, rather than assigning a wall-clock
    // performance requirement to synchronous immutable-object persistence.
    for pass in 0..24 {
        missing_body_one_pass(state, session_id).await;
        let conversations = wardian_core::paths::agent_conversations_dir(session_id).unwrap();
        let directory = conversations.parent().unwrap();
        let reference: String =
            serde_json::from_slice(&std::fs::read(directory.join("chat-read-head.json")).unwrap())
                .unwrap();
        let head: serde_json::Value = serde_json::from_slice(
            &std::fs::read(directory.join("chat-read-objects").join(&reference)).unwrap(),
        )
        .unwrap();
        eprintln!(
            "normal checkpoint: {}",
            serde_json::json!({"pass": pass, "head": reference, "progress": head["progress"], "narrative_before": head["bootstrap"]["narrative_before"], "event_before": head["bootstrap"]["event_before"], "body_before": head["bootstrap"]["body_before"]})
        );
        if head["progress"] == "ready" {
            break;
        }
        assert!(
            pass < 23,
            "saved fixture must reach ready within its bounded normal passes"
        );
    }
    let request = crate::commands::chat::background_capture_request(state, session_id)
        .await
        .unwrap()
        .unwrap();
    let claim =
        crate::commands::background_capture::admit_background_capture(state, request, false)
            .await
            .unwrap();
    crate::commands::background_capture::run_background_capture(state, claim).await;
}

async fn missing_body_page(
    state: &AppState,
    session_id: &str,
    cursor: Option<String>,
) -> wardian_core::models::chat::AgentChatPage {
    crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        state,
        session_id.into(),
        cursor,
        None,
        None,
        None,
    )
    .await
    .unwrap()
}

async fn missing_body_details(
    state: &AppState,
    session_id: &str,
    row: &wardian_core::models::chat::AgentChatEvent,
    complete: bool,
) -> String {
    let mut next = row.metadata["chat_detail_ref"].as_str().map(str::to_owned);
    let mut text = String::new();
    for _ in 0..16 {
        let Some(reference) = next.take() else { break };
        let page = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
            state,
            session_id.into(),
            None,
            None,
            Some(reference),
            None,
        )
        .await
        .unwrap();
        let detail = page.detail.unwrap();
        assert_eq!(detail.event_id, row.id);
        if !complete {
            assert!(
                !detail.complete,
                "unavailable suffix must not claim a complete body"
            );
        }
        if detail.next.is_none() {
            assert_eq!(detail.complete, complete);
        }
        text.push_str(&detail.text);
        next = detail.next;
    }
    assert!(
        next.is_none(),
        "detail availability must have a finite boundary"
    );
    text
}

#[tokio::test]
async fn cold_missing_body_complete_control() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    let (config, original, _, body, saved) = missing_body_fixture(&state, &home, 133).await;
    missing_body_one_pass(&state, &config.session_id).await;
    let seed = missing_body_page(&state, &config.session_id, None).await;
    assert_eq!(seed.conversation_id.as_deref(), Some(original.as_str()));
    assert_eq!(seed.events.len(), 24);
    missing_body_resume(&state, &config.session_id).await;
    let older = missing_body_page(&state, &config.session_id, seed.next_before).await;
    assert_eq!(older.generation, seed.generation);
    let row = older
        .events
        .iter()
        .find(|event| event.id == "missing-body-133")
        .expect("older artifact header");
    assert_eq!(
        missing_body_details(&state, &config.session_id, row, true).await,
        body
    );
    for (path, bytes) in saved {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn cold_missing_body_other_io_remains_failure() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    let (config, _, artifact, _, _) = missing_body_fixture(&state, &home, 133).await;
    assert!(artifact
        .canonicalize()
        .unwrap()
        .starts_with(home.path().canonicalize().unwrap()));
    std::fs::remove_file(&artifact).unwrap();
    std::fs::create_dir(&artifact).unwrap();
    missing_body_one_pass(&state, &config.session_id).await;
    let seed = missing_body_page(&state, &config.session_id, None).await;
    let request = crate::commands::chat::background_capture_request(&state, &config.session_id)
        .await
        .unwrap()
        .unwrap();
    let claim =
        crate::commands::background_capture::admit_background_capture(&state, request, false)
            .await
            .unwrap();
    let result =
        crate::commands::chat::archive_background_capture_pass(&state, &claim.request).await;
    drop(claim);
    assert!(
        result.is_err(),
        "an artifact directory/permission error must remain observable"
    );
    let unchanged = missing_body_page(&state, &config.session_id, None).await;
    assert_eq!(unchanged.next_before, seed.next_before);
    assert_eq!(unchanged.events.len(), seed.events.len());
    assert!(unchanged
        .events
        .iter()
        .all(|event| event.metadata["chat_body_unavailable"] != true));
}

#[tokio::test]
async fn cold_missing_body_before_admission_preserves_older_headers() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    let (config, original, artifact, _, saved) = missing_body_fixture(&state, &home, 133).await;
    assert!(artifact
        .canonicalize()
        .unwrap()
        .starts_with(home.path().canonicalize().unwrap()));
    std::fs::remove_file(&artifact).unwrap();
    missing_body_one_pass(&state, &config.session_id).await;
    let seed = missing_body_page(&state, &config.session_id, None).await;
    assert_eq!(seed.events.len(), 24);
    missing_body_one_pass(&state, &config.session_id).await;
    missing_body_resume(&state, &config.session_id).await;
    let older = missing_body_page(&state, &config.session_id, seed.next_before.clone()).await;
    assert_eq!(older.conversation_id.as_deref(), Some(original.as_str()));
    assert_eq!(older.generation, seed.generation);
    assert_ne!(
        older.next_before, seed.next_before,
        "older history must advance past the missing body"
    );
    let row = older
        .events
        .iter()
        .find(|event| event.id == "missing-body-133")
        .expect("missing body header preserved");
    assert_eq!(row.metadata["chat_body_unavailable"], true);
    assert_ne!(row.metadata["chat_body_pending"], true);
    assert!(row.metadata.get("chat_detail_ref").is_none());
    for (path, bytes) in saved {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn cold_missing_body_after_prefix_preserves_committed_chunks() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    let (config, original, artifact, body, saved) = missing_body_fixture(&state, &home, 160).await;
    missing_body_one_pass(&state, &config.session_id).await;
    let seed = missing_body_page(&state, &config.session_id, None).await;
    let row = seed
        .events
        .iter()
        .find(|event| event.id == "missing-body-160")
        .unwrap();
    assert_eq!(row.metadata["chat_body_pending"], true);
    assert!(
        row.metadata["chat_detail_ref"].is_string(),
        "first body prefix was committed"
    );
    assert!(artifact
        .canonicalize()
        .unwrap()
        .starts_with(home.path().canonicalize().unwrap()));
    std::fs::remove_file(&artifact).unwrap();
    missing_body_one_pass(&state, &config.session_id).await;
    missing_body_resume(&state, &config.session_id).await;
    let recent = missing_body_page(&state, &config.session_id, None).await;
    assert_eq!(recent.conversation_id.as_deref(), Some(original.as_str()));
    assert_eq!(recent.generation, seed.generation);
    let row = recent
        .events
        .iter()
        .find(|event| event.id == "missing-body-160")
        .unwrap();
    assert_eq!(row.metadata["chat_body_unavailable"], true);
    assert_ne!(row.metadata["chat_body_pending"], true);
    assert_eq!(
        missing_body_details(&state, &config.session_id, row, false).await,
        body[..64 * 1024]
    );
    let older = missing_body_page(&state, &config.session_id, seed.next_before).await;
    assert!(!older.events.is_empty());
    for (path, bytes) in saved {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

async fn cold_scope_partial_fixture(
    state: &AppState,
    home: &crate::control::test_support::TestWardianHome,
    with_sessions: bool,
) -> (wardian_core::models::AgentConfig, String) {
    use wardian_core::models::{AgentConfig, ProviderConfig};
    let workspace = home.path().join("scope-workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut config = AgentConfig {
        session_id: "cold-scope-agent".into(),
        session_name: "Cold scope".into(),
        provider: "codex".into(),
        is_off: true,
        folder: workspace.to_string_lossy().into_owned(),
        model: None,
        provider_config: ProviderConfig::Codex(Default::default()),
        conversation_logging: wardian_core::conversations::AgentConversationLoggingSetting::Enabled,
        ..Default::default()
    };
    if let ProviderConfig::Codex(value) = &mut config.provider_config {
        value.reasoning_effort = None;
    }
    if with_sessions {
        config.resume_session = Some("scope-primary".into());
        config.fresh_provider_session_id = Some("scope-secondary-one".into());
    }
    register_cold_scope_agent(state, &config).await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(state, &config.session_id)
        .await
        .unwrap();
    assert!(
        snapshot.log_path.is_none(),
        "fixture must never bind a real vendor log"
    );
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let fixture = crate::state::conversation_archive::ConversationArchiveState::default();
    let events = (0..64)
        .map(|i| {
            let text = if i == 63 {
                "old scoped body ".repeat(700)
            } else {
                format!("old scoped reply {i}")
            };
            serde_json::from_value(serde_json::json!({
        "id": format!("cold-scope-old-{i}"), "session_id": config.session_id, "provider": "codex",
        "kind": "message", "role": "assistant", "text": text,
        "metadata": {"provider_log": true}
    })).unwrap()
        })
        .collect::<Vec<wardian_core::models::chat::AgentChatEvent>>();
    fixture
        .append_chat_events_with_context(context, &events)
        .unwrap();
    let original = fixture
        .active_conversation_id(&config.session_id)
        .unwrap()
        .unwrap();
    let directory = wardian_core::paths::agent_conversations_dir(&config.session_id).unwrap();
    let pointer = directory.parent().unwrap().join("chat-read-head.json");
    let objects = directory.parent().unwrap().join("chat-read-objects");
    assert!(pointer.starts_with(home.path()) && objects.starts_with(home.path()));
    if pointer.exists() {
        std::fs::remove_file(pointer).unwrap();
    }
    if objects.exists() {
        std::fs::remove_dir_all(objects).unwrap();
    }
    let request = crate::commands::chat::background_capture_request(state, &config.session_id)
        .await
        .unwrap()
        .unwrap();
    let claim = crate::commands::background_capture::admit_background_capture(state, request, true)
        .await
        .unwrap();
    let stop = crate::commands::chat::archive_background_capture_pass(state, &claim.request)
        .await
        .unwrap();
    assert!(matches!(
        stop,
        crate::state::background_capture::CaptureStop::More
    ));
    drop(claim); // Retain the ordinary coordinator's retry intent at a partial checkpoint.
    (config, original)
}

#[tokio::test]
async fn cold_scope_public_config_change_rejects_none_key_roots_before_unchanged_and_after_restart()
{
    run_cold_scope_public_config_change(false).await;
}

#[tokio::test]
async fn cold_scope_secondary_session_change_rejects_same_primary_key_before_unchanged_and_after_restart(
) {
    run_cold_scope_public_config_change(true).await;
}

async fn run_cold_scope_public_config_change(with_sessions: bool) {
    use tauri::Manager;
    use wardian_core::models::ProviderConfig;
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let (config, original) = cold_scope_partial_fixture(&state, &home, with_sessions).await;
    let first = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        config.session_id.clone(),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(first.conversation_id.as_deref(), Some(original.as_str()));
    assert!(!first.events.is_empty());
    let old_detail = first
        .events
        .iter()
        .find_map(|row| row.metadata["chat_detail_ref"].as_str())
        .expect("partial fixture publishes an owned body detail")
        .to_owned();
    let before = crate::commands::chat::agent_archive_capture_snapshot(&state, &config.session_id)
        .await
        .unwrap();
    let old_context = crate::commands::chat::conversation_archive_context_from_snapshot(&before);
    let incarnation = state.agents.lock().await[&config.session_id]
        .current_status
        .clone();
    let mut updated = config.clone();
    if with_sessions {
        updated.fresh_provider_session_id = Some("scope-secondary-two".into());
    } else {
        updated.provider = "claude".into();
        updated.provider_config = ProviderConfig::Claude(Default::default());
        if let ProviderConfig::Claude(value) = &mut updated.provider_config {
            value.reasoning_effort = None;
        }
    }
    // The real complete public command and persistence delegate, with model
    // and effort unchanged so it never invokes provider catalog discovery.
    let result =
        crate::commands::agent::update_agent_config(updated, app.state(), app.handle().clone())
            .await
            .unwrap();
    assert!(Arc::ptr_eq(
        &incarnation,
        &state.agents.lock().await[&config.session_id].current_status
    ));
    let after = crate::commands::chat::agent_archive_capture_snapshot(&state, &config.session_id)
        .await
        .unwrap();
    let new_context = crate::commands::chat::conversation_archive_context_from_snapshot(&after);
    assert_eq!(
        old_context.provider_source_key, new_context.provider_source_key,
        "source-key equality must not stand in for full scope"
    );
    let immediate = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        config.session_id.clone(),
        None,
        Some(first.revision.clone()),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        immediate.reset && !immediate.unchanged,
        "full scope change must retire last-good rows before unchanged fast path"
    );
    assert_ne!(immediate.revision, first.revision);
    assert!(immediate.conversation_id.is_none());
    assert!(immediate
        .events
        .iter()
        .all(|row| !row.id.starts_with("cold-scope-old-")));
    let detail = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        config.session_id.clone(),
        None,
        Some(first.revision.clone()),
        Some(old_detail.clone()),
        None,
    )
    .await
    .unwrap();
    assert!(detail.reset && !detail.unchanged && detail.detail.is_none());
    assert!(detail.events.is_empty() && detail.conversation_id.is_none());
    assert_ne!(detail.revision, first.revision);
    // Restart before either owner gets a chance to retire the stored checkpoint.
    let restored = AppState::new();
    register_cold_scope_agent(&restored, &result.config).await;
    let restarted = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &restored,
        config.session_id.clone(),
        None,
        Some(first.revision.clone()),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(restarted.reset && !restarted.unchanged);
    assert!(restarted.events.is_empty() && restarted.conversation_id.is_none());
    assert_ne!(restarted.revision, first.revision);
    let restarted_detail = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &restored,
        config.session_id.clone(),
        None,
        Some(first.revision),
        Some(old_detail),
        None,
    )
    .await
    .unwrap();
    assert!(restarted_detail.reset && restarted_detail.detail.is_none());
    crate::commands::background_capture::capture_background_for_state(&state, &config.session_id)
        .await
        .unwrap();
    let ordinary = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        config.session_id.clone(),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(ordinary.conversation_id.is_none());
    assert!(ordinary
        .events
        .iter()
        .all(|row| !row.id.starts_with("cold-scope-old-")));
    crate::commands::background_capture::capture_background_for_state(
        &restored,
        &config.session_id,
    )
    .await
    .unwrap();
    let final_page = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &restored,
        config.session_id,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(final_page.conversation_id.is_none());
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
async fn ordinary_off_provisional_recent_page_becomes_admitted_after_capture() {
    const ROW_COUNT: usize = 240;

    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    assert_eq!(dispatch_ordinary_capture(&state).await, 1);

    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    let unknown_prefix_end = std::fs::metadata(&path).unwrap().len();
    let primed = capture_state(&state, &snapshot);
    assert_eq!(primed.committed_offset, unknown_prefix_end);
    assert_eq!(primed.unknown_before_offset, Some(unknown_prefix_end));
    assert_eq!(primed.status, "complete");
    assert_eq!(
        state
            .conversation_archive
            .active_conversation_id(&session_id)
            .unwrap(),
        None,
        "priming the unknown prefix does not admit a conversation"
    );

    let mut provider_log = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    for index in 0..ROW_COUNT {
        let record = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "turn_id": format!("page-turn-{index}"),
                "message": format!("Bounded page history {index}"),
            }
        });
        writeln!(provider_log, "{record}").unwrap();
    }
    drop(provider_log);
    let appended_end = std::fs::metadata(&path).unwrap().len();

    let source_index_dir = wardian_core::paths::agent_conversations_dir(&session_id)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let read_source_head = || {
        let reference: Option<String> = serde_json::from_slice(
            &std::fs::read(source_index_dir.join("chat-source-head.json")).unwrap(),
        )
        .unwrap();
        let reference = reference.expect("ordinary capture publishes a source-index head");
        let head: serde_json::Value = serde_json::from_slice(
            &std::fs::read(source_index_dir.join("chat-read-objects").join(&reference)).unwrap(),
        )
        .unwrap();
        (reference, head)
    };
    let assert_source_head_prefix = |head: &serde_json::Value| {
        assert_eq!(head["agent_id"].as_str(), Some(session_id.as_str()));
        assert_eq!(
            head["target_extent"].as_u64(),
            Some(unknown_prefix_end),
            "source index remains pinned at the primed capture prefix"
        );
        assert_eq!(
            head["floor"].as_u64(),
            primed.unknown_before_offset,
            "source-index floor matches the captured unknown-prefix boundary"
        );
        assert!(
            head["root"].is_null(),
            "no source rows are rooted at the primed prefix"
        );
    };
    let (primed_source_reference, primed_source_head) = read_source_head();
    assert_source_head_prefix(&primed_source_head);

    let provisional = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        session_id.clone(),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (source_reference_after_page, source_head_after_page) = read_source_head();
    assert_eq!(source_reference_after_page, primed_source_reference);
    assert_eq!(source_head_after_page, primed_source_head);
    assert_source_head_prefix(&source_head_after_page);
    assert_eq!(provisional.events.len(), 80);
    assert_eq!(
        provisional
            .events
            .first()
            .and_then(|event| event.text.as_deref()),
        Some("Bounded page history 160")
    );
    assert_eq!(
        provisional
            .events
            .last()
            .and_then(|event| event.text.as_deref()),
        Some("Bounded page history 239")
    );
    assert_eq!(
        provisional
            .events
            .last()
            .and_then(|event| event.turn_id.as_deref()),
        Some("page-turn-239")
    );
    assert!(provisional.conversation_id.is_none());
    assert_eq!(
        provisional.generation.as_deref(),
        primed_source_head["generation"].as_str(),
        "the provisional page is scoped to the actual source-index generation"
    );
    assert_eq!(
        provisional.source_epoch.as_deref(),
        primed_source_head["epoch"].as_str(),
        "the provisional page is scoped to the actual source-index epoch"
    );
    assert_ne!(provisional.progress, "ready");
    assert!(provisional.events.iter().all(|event| {
        event.metadata["chat_provisional"] == true
            && event.metadata["chat_identity_resolution"] == "unresolved"
            && event.metadata["chat_source_ref"].as_str().is_some()
            && event.metadata["chat_source_admission"].as_str().is_some()
    }));
    let still_primed = capture_state(&state, &snapshot);
    assert_eq!(still_primed.committed_offset, unknown_prefix_end);
    assert_eq!(still_primed.unknown_before_offset, Some(unknown_prefix_end));
    assert_eq!(still_primed.status, "complete");
    assert!(
        state
            .conversation_archive
            .chat_events_for_agent(&session_id)
            .unwrap()
            .is_empty(),
        "the provisional page read must leave the archive index at the primed prefix"
    );

    assert_eq!(dispatch_ordinary_capture(&state).await, 1);
    let captured = capture_state(&state, &snapshot);
    assert_eq!(captured.committed_offset, appended_end);
    assert_eq!(captured.unknown_before_offset, Some(unknown_prefix_end));
    assert_eq!(captured.status, "complete");

    let admitted = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        session_id.clone(),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (_admitted_source_reference, admitted_source_head) = read_source_head();
    let source_context =
        crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let expected_source_key = source_context.provider_source_key.as_deref();
    let observed_source_epoch = admitted.source_epoch.as_deref();
    let observed_source_admission = admitted_source_head["admission"].as_str();
    let root_present = admitted_source_head["root"]
        .as_str()
        .is_some_and(|root| !root.is_empty());
    let metadata_keys = admitted
        .events
        .first()
        .and_then(|event| event.metadata.as_object())
        .map(|metadata| metadata.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let rows_with_admission = admitted
        .events
        .iter()
        .filter(|event| {
            event
                .metadata
                .get("chat_source_admission")
                .is_some_and(|value| !value.is_null())
        })
        .count();
    eprintln!(
        "ordinary_off_admission progress={} session_matches={} conversation_present={} generation_matches_head={} epoch_matches_head={} owner_matches={} source_key_matches={} target_extent={:?} appended_frontier={} floor={:?} root_present={} admission_present={} rows={} row_admissions_present={} metadata_keys={:?}",
        admitted.progress,
        admitted.session_id == session_id,
        admitted.conversation_id.as_deref().is_some_and(|id| !id.is_empty()),
        admitted.generation.as_deref() == admitted_source_head["generation"].as_str(),
        observed_source_epoch == admitted_source_head["epoch"].as_str(),
        admitted_source_head["agent_id"].as_str() == Some(session_id.as_str()),
        admitted_source_head["source_key"].as_str().is_some_and(|key| Some(key) == expected_source_key),
        admitted_source_head["target_extent"].as_u64(),
        appended_end,
        admitted_source_head["floor"].as_u64(),
        root_present,
        observed_source_admission.is_some_and(|value| !value.is_empty()),
        admitted.events.len(),
        rows_with_admission,
        metadata_keys,
    );
    assert_eq!(admitted.session_id, session_id);
    assert!(
        matches!(admitted.progress.as_str(), "ready" | "provisional"),
        "postcapture page must reach ready or settled provisional progress"
    );
    assert!(admitted
        .conversation_id
        .as_deref()
        .is_some_and(|id| !id.is_empty()));
    assert_eq!(
        admitted_source_head["agent_id"].as_str(),
        Some(session_id.as_str())
    );
    assert_eq!(
        admitted_source_head["source_key"].as_str(),
        expected_source_key
    );
    assert_eq!(
        admitted_source_head["epoch"].as_str(),
        observed_source_epoch
    );
    assert_eq!(
        admitted_source_head["target_extent"].as_u64(),
        Some(appended_end)
    );
    assert_eq!(
        admitted_source_head["floor"].as_u64(),
        primed.unknown_before_offset
    );
    assert!(
        root_present,
        "postcapture source index has an admitted root"
    );
    let source_epoch = observed_source_epoch.expect("admitted page source epoch");
    let source_admission = observed_source_admission
        .filter(|admission| !admission.is_empty())
        .expect("published source head admission");
    assert_eq!(
        admitted.generation.as_deref(),
        admitted_source_head["generation"].as_str()
    );
    assert_eq!(
        admitted_source_head["progress"].as_str(),
        Some(admitted.progress.as_str())
    );
    assert_eq!(admitted.events.len(), 80);
    assert_eq!(
        admitted
            .events
            .first()
            .and_then(|event| event.text.as_deref()),
        Some("Bounded page history 160")
    );
    assert_eq!(
        admitted
            .events
            .last()
            .and_then(|event| event.text.as_deref()),
        Some("Bounded page history 239")
    );
    assert_eq!(
        admitted
            .events
            .last()
            .and_then(|event| event.turn_id.as_deref()),
        Some("page-turn-239")
    );
    assert!(!admitted.reset);
    assert!(admitted.conversation_id.is_some());
    assert!(admitted
        .generation
        .as_deref()
        .is_some_and(|value| !value.is_empty()));
    assert!(admitted.events.iter().all(|event| {
        event.metadata["chat_source_epoch"].as_str() == Some(source_epoch)
            && event
                .metadata
                .get("chat_source_admission")
                .is_none_or(|value| value.is_null() || value.as_str() == Some(source_admission))
    }));
    assert!(
        admitted
            .next_before
            .as_deref()
            .is_some_and(|cursor| !cursor.is_empty()),
        "the admitted page issues its opaque older cursor"
    );
    assert_eq!(
        state
            .conversation_archive
            .active_conversation_id(&session_id)
            .unwrap(),
        admitted.conversation_id
    );

    let mut requested_ids = Vec::new();
    let mut seen_source_ids = std::collections::HashSet::<String>::new();
    for event in &admitted.events {
        if event.metadata["optimistic"] == true {
            continue;
        }
        let mut row_source_ids: Vec<String> = Vec::new();
        if let Some(members) = event.metadata["chat_display_member_ids"].as_array() {
            for member in members {
                if let Some(id) = member
                    .as_str()
                    .filter(|id| id.starts_with("source:") && id.len() > 7)
                {
                    if !row_source_ids
                        .iter()
                        .any(|existing| existing.as_str() == id)
                    {
                        row_source_ids.push(id.to_owned());
                    }
                }
            }
        }
        for id in [
            event.metadata["chat_source_ref"].as_str(),
            event.metadata["chat_display_physical_id"].as_str(),
            Some(event.id.as_str()),
        ]
        .into_iter()
        .flatten()
        .filter(|id| id.starts_with("source:") && id.len() > 7)
        {
            if !row_source_ids
                .iter()
                .any(|existing| existing.as_str() == id)
            {
                row_source_ids.push(id.to_owned());
            }
        }
        assert!(
            row_source_ids.len() <= 2,
            "consumer permits at most two source IDs per row"
        );
        for id in row_source_ids {
            if seen_source_ids.insert(id.clone()) {
                requested_ids.push(id);
            }
            if requested_ids.len() == 8 {
                break;
            }
        }
        if requested_ids.len() == 8 {
            break;
        }
    }
    assert_eq!(requested_ids.len(), 8);
    assert_eq!(
        requested_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        8,
        "the normal renewal request contains eight unique page IDs"
    );
    let renewed = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        session_id.clone(),
        None,
        Some(admitted.revision.clone()),
        None,
        Some(requested_ids.clone()),
    )
    .await
    .unwrap();
    eprintln!(
        "ordinary_off_admission renewal cursor_present={} ack_count={} ack_matches={} progress_matches={} scope_matches={} revision_matches={}",
        admitted.next_before.as_deref().is_some_and(|cursor| !cursor.is_empty()),
        renewed.revalidated_ids.as_ref().map_or(0, |ids| ids.len()),
        renewed.revalidated_ids.as_deref() == Some(requested_ids.as_slice()),
        renewed.progress == admitted.progress,
        renewed.session_id == admitted.session_id
            && renewed.conversation_id.as_deref() == admitted.conversation_id.as_deref()
            && renewed.generation.as_deref() == admitted.generation.as_deref()
            && renewed.source_epoch.as_deref() == admitted.source_epoch.as_deref(),
        renewed.revision == admitted.revision,
    );
    assert_eq!(renewed.session_id, session_id);
    assert_eq!(
        renewed.revalidated_ids.as_deref(),
        Some(requested_ids.as_slice())
    );
    assert!(!renewed.reset);
    assert_eq!(renewed.revision, admitted.revision);
    assert_eq!(
        renewed.conversation_id.as_deref(),
        admitted.conversation_id.as_deref()
    );
    assert_eq!(
        renewed.generation.as_deref(),
        admitted.generation.as_deref()
    );
    assert_eq!(
        renewed.source_epoch.as_deref(),
        admitted.source_epoch.as_deref()
    );
    let roster = state.agents.lock().await;
    let agent = roster.get(&session_id).unwrap();
    assert_eq!(*agent.current_status.lock().unwrap(), "Off");
    assert!(
        agent.runtime_generation.is_none(),
        "no provider was submitted"
    );
}

#[tokio::test]
async fn ordinary_off_page_retains_commentary_while_oversized_append_is_unindexed() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session_id, path) = registered_off_source(&home).await;
    assert_eq!(dispatch_ordinary_capture(&state).await, 1);
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session_id)
        .await
        .unwrap();
    let header_end = std::fs::metadata(&path).unwrap().len();
    let primed_capture = capture_state(&state, &snapshot);
    assert_eq!(primed_capture.committed_offset, header_end);
    assert_eq!(primed_capture.unknown_before_offset, Some(header_end));
    assert_eq!(primed_capture.status, "complete");
    let source_identity = crate::commands::provider_log_acquisition::native_file_identity(
        &std::fs::File::open(&path).unwrap(),
    )
    .unwrap();
    let (epoch, admission, scoped_extent, _, source_policy) =
        crate::commands::chat_recent_seed::source_scope_with_policy(&snapshot).unwrap();
    assert_eq!(scoped_extent, header_end);
    assert!(source_policy.valid);
    assert_eq!(source_policy.agent_id, session_id);
    assert_eq!(source_policy.unknown_before, header_end);
    assert!(!source_policy.source_key.is_empty());
    assert!(!epoch.is_empty());
    assert!(!admission.is_empty());

    let source_index_dir = wardian_core::paths::agent_conversations_dir(&session_id)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let read_source_head = || {
        let reference: Option<String> = serde_json::from_slice(
            &std::fs::read(source_index_dir.join("chat-source-head.json")).unwrap(),
        )
        .unwrap();
        let reference = reference.expect("ordinary header capture publishes its source-index head");
        let head: serde_json::Value = serde_json::from_slice(
            &std::fs::read(source_index_dir.join("chat-read-objects").join(&reference)).unwrap(),
        )
        .unwrap();
        (reference, head)
    };
    let primed_head = read_source_head();
    assert_eq!(
        primed_head.1["agent_id"].as_str(),
        Some(session_id.as_str())
    );
    assert_eq!(primed_head.1["target_extent"].as_u64(), Some(header_end));
    assert_eq!(primed_head.1["floor"].as_u64(), Some(header_end));
    assert!(primed_head.1["root"].is_null());
    assert_eq!(primed_head.1["epoch"].as_str(), Some(epoch.as_str()));
    assert!(primed_head.1["admission"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert!(state
        .conversation_archive
        .chat_events_for_agent(&session_id)
        .unwrap()
        .is_empty());

    let pair_text = "A source-bound commentary observation.";
    let context_line = serde_json::json!({
        "type": "turn_context",
        "payload": {"turn_id": "commentary-provider-turn"}
    })
    .to_string();
    let stream_line = serde_json::json!({
        "timestamp": "2026-10-24T17:38:18.275Z",
        "type": "event_msg",
        "payload": {"type": "agent_message", "phase": "commentary", "message": pair_text}
    })
    .to_string();
    let item_line = serde_json::json!({
        "timestamp": "2026-10-24T17:38:18.275Z",
        "type": "response_item",
        "payload": {
            "type": "message",
            "id": "msg-off-oversized-mirror",
            "role": "assistant",
            "phase": "commentary",
            "content": [{"type": "output_text", "text": pair_text}],
            "internal_chat_message_metadata_passthrough": {
                "turn_id": "commentary-provider-turn",
                "create_time": "2026-10-24T17:38:18.275Z",
                "content_item_kinds": ["unknown"]
            }
        }
    })
    .to_string();
    let output_line = serde_json::json!({
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call_output",
            "call_id": "off-oversized-complete-output",
            "output": "x".repeat(17 * 1024)
        }
    })
    .to_string();
    assert!(output_line.len() > 16 * 1024);
    assert!(output_line.len() < 128 * 1024);
    assert!(serde_json::from_str::<serde_json::Value>(&output_line).is_ok());
    let trailing_bookkeeping = serde_json::json!({
        "type": "turn_context",
        "payload": {"turn_id": "bookkeeping-after-oversized-output"}
    })
    .to_string();
    let mut appended = String::new();
    appended.push_str(&context_line);
    appended.push('\n');
    let stream_start = header_end + appended.len() as u64;
    let stream_end = stream_start + stream_line.len() as u64 + 1;
    appended.push_str(&stream_line);
    appended.push('\n');
    let item_start = header_end + appended.len() as u64;
    let item_end = item_start + item_line.len() as u64 + 1;
    appended.push_str(&item_line);
    appended.push('\n');
    appended.push_str(&output_line);
    appended.push('\n');
    appended.push_str(&trailing_bookkeeping);
    appended.push('\n');
    assert!(appended.len() < 128 * 1024);

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(appended.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let appended_end = std::fs::metadata(&path).unwrap().len();
    assert_eq!(appended_end, header_end + appended.len() as u64);
    assert_eq!(
        crate::commands::provider_log_acquisition::native_file_identity(
            &std::fs::File::open(&path).unwrap(),
        )
        .unwrap(),
        source_identity,
        "the source was extended in place"
    );

    let recent = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    assert_eq!(recent.progress, "oversized_record");
    assert_eq!(recent.events.len(), 2);
    assert!(recent.bytes_read < 2 * 1024 * 1024);
    assert!(recent.records_decoded <= 84);
    assert!(recent
        .checkpoint
        .as_ref()
        .is_some_and(|checkpoint| !checkpoint.window_complete && checkpoint.normalizer_complete));
    let (current_epoch, current_admission, current_extent, _, _) =
        crate::commands::chat_recent_seed::source_scope_with_policy(&snapshot).unwrap();
    assert_eq!(current_epoch, epoch);
    assert_eq!(current_admission, admission);
    assert_eq!(current_extent, appended_end);

    let page = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        session_id.clone(),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(page.session_id, session_id);
    assert_eq!(page.progress, "oversized_record");
    assert!(!page.reset);
    assert!(page.conversation_id.is_none());
    assert_eq!(
        page.generation.as_deref(),
        primed_head.1["generation"].as_str()
    );
    assert_eq!(page.source_epoch.as_deref(), Some(epoch.as_str()));
    assert!(
        page.revalidated_ids.is_none(),
        "a read-only page load is not a revalidation ACK"
    );
    assert!(
        page.next_before.is_none(),
        "an incomplete oversized window has no older cursor"
    );
    assert_eq!(page.events.len(), 2);
    assert!(page.bytes_read < 2 * 1024 * 1024);
    assert!(
        serde_json::to_vec(&page).unwrap().len()
            <= crate::state::conversation_archive::chat_read::IPC_BYTES
    );

    for (source, start, end, native_turn_id) in [
        ("event_msg", stream_start, stream_end, None),
        (
            "response_item",
            item_start,
            item_end,
            Some("msg-off-oversized-mirror"),
        ),
    ] {
        let matching: Vec<_> = page
            .events
            .iter()
            .filter(|event| event.source.as_deref() == Some(source))
            .collect();
        assert_eq!(matching.len(), 1, "one visible {source} observation");
        let event = matching[0];
        let source_ref = event.metadata["chat_source_ref"]
            .as_str()
            .expect("the visible row retains its source reference");
        assert_eq!(event.id, source_ref);
        assert_eq!(
            event.kind,
            wardian_core::models::chat::AgentChatEventKind::Message
        );
        assert_eq!(
            event.role,
            Some(wardian_core::models::chat::AgentChatRole::Assistant)
        );
        assert_eq!(event.turn_id.as_deref(), native_turn_id);
        assert_eq!(event.text.as_deref(), Some(pair_text));
        assert_eq!(event.metadata["chat_source_start"].as_u64(), Some(start));
        assert_eq!(event.metadata["chat_source_end"].as_u64(), Some(end));
        assert_eq!(
            event.metadata["chat_source_epoch"].as_str(),
            Some(epoch.as_str())
        );
        assert_eq!(
            event.metadata["chat_source_admission"].as_str(),
            Some(admission.as_str())
        );
        assert_eq!(event.metadata["chat_provisional"], serde_json::json!(true));
        assert_eq!(
            event.metadata["chat_identity_resolution"],
            serde_json::json!("unresolved")
        );
        assert_eq!(
            event.metadata["provider_turn_id"].as_str(),
            Some("commentary-provider-turn")
        );
        assert!(
            event.text.as_ref().unwrap().len()
                <= crate::state::conversation_archive::chat_read::PREVIEW_BYTES
        );
    }
    assert_eq!(
        read_source_head(),
        primed_head,
        "the source-index head remains at the primed header"
    );
    let still_primed = capture_state(&state, &snapshot);
    assert_eq!(still_primed.committed_offset, header_end);
    assert_eq!(still_primed.unknown_before_offset, Some(header_end));
    assert_eq!(still_primed.status, "complete");
    assert!(state
        .conversation_archive
        .chat_events_for_agent(&session_id)
        .unwrap()
        .is_empty());
    let roster = state.agents.lock().await;
    let agent = roster.get(&session_id).unwrap();
    assert_eq!(*agent.current_status.lock().unwrap(), "Off");
    assert!(agent.runtime_generation.is_none());
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
