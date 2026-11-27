use super::*;
use std::future::{poll_fn, Future};
use std::io::Write;
use std::task::Poll;

#[cfg(feature = "adoption-throughput")]
#[path = "adoption_throughput_tests.rs"]
mod adoption_throughput_tests;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_archive_gap_blocks_forced_and_changed_source_capture_and_reaches_chat() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_off_source(&home).await;
    *state.agents.lock().await[&session].log_path.lock().unwrap() = Some(path.clone());
    let gap = wardian_core::conversations::ArchiveSequenceGap {
        conversation_id: "damaged-conversation".into(),
        provider_source_key: Some("codex:session:off-provider-conversation".into()),
        sequence: 2,
        vacancy_rule: None,
    };
    let status_path = home
        .path()
        .join("agents")
        .join(&session)
        .join("archive-capture-stop.json");
    let bytes = serde_json::to_vec(&gap).unwrap();
    std::fs::write(&status_path, &bytes).unwrap();
    for force in [false, true] {
        append_records(&path, 1);
        let request = crate::commands::chat::background_capture_request(&state, &session)
            .await
            .unwrap()
            .unwrap();
        assert!(
            crate::commands::background_capture::admit_background_capture(
                &state,
                request.clone(),
                force
            )
            .await
            .is_none()
        );
        assert_eq!(
            crate::commands::chat::archive_background_capture_pass(&state, &request)
                .await
                .unwrap(),
            crate::state::background_capture::CaptureStop::Retired
        );
    }
    let error = crate::commands::chat::load_agent_chat_page_with_preview_for_state(
        &state,
        session,
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error, gap.reason());
    assert_eq!(std::fs::read(status_path).unwrap(), bytes);
    assert!(!home
        .path()
        .join("agents/ordinary-off-capture/conversations")
        .exists());
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
    assert!(
        !state
            .conversation_archive
            .chat_projection
            .has_pending_capture_work(&session),
        "an adoption chunk must defer warm body hydration to gated saved-archive replay"
    );
    drop(worker);
    let (_, retry) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    status::dispatch_capture_follow_up(&state, retry.background_captures, |claim| {
        claims.push(claim)
    })
    .await;
    assert_eq!(
        claims.len(),
        1,
        "a cancelled partial import retries normally"
    );
    let mut worker = Box::pin(crate::commands::background_capture::run_background_capture(
        &state,
        claims.pop().unwrap(),
    ));
    state
        .conversation_archive
        .append_delivered_input_with_context(
            context.clone(),
            "Normal delivered prompt during archive adoption",
            None,
        )
        .unwrap();
    poll_fn(|cx| {
        assert!(worker.as_mut().poll(cx).is_pending());
        let work: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&work_path).unwrap()).unwrap();
        if work["status"] == "complete" {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    assert!(!state
        .conversation_archive
        .has_pending_adoption(&session)
        .unwrap());
    assert!(!state
        .conversation_archive
        .chat_projection
        .has_pending_capture_work(&session));
    // Completion still owes its final More: retry the cancelled claim through
    // ordinary telemetry while both adoption and warm projection demand are empty.
    drop(worker);
    let (_, retry) = collect_agent_metrics_with_sampler(&state, stopped_sample).await;
    status::dispatch_capture_follow_up(&state, retry.background_captures, |claim| {
        claims.push(claim)
    })
    .await;
    assert_eq!(
        claims.len(),
        1,
        "final replay retries without a forced command"
    );
    state
        .conversation_archive
        .append_delivered_input_with_context(
            context.clone(),
            "Normal delivered prompt after the final adoption chunk",
            None,
        )
        .unwrap();
    crate::commands::background_capture::run_background_capture(&state, claims.pop().unwrap())
        .await;
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
    let expected_epoch = {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&replaced_receipt.native_identity).unwrap())
        )
    };
    for page_index in 0..8 {
        let page = crate::commands::chat::load_agent_chat_page_with_preview_for_state(
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
        assert_eq!(page.progress, "archive_history_unverified");
        assert_eq!(page.source_epoch.as_deref(), Some(expected_epoch.as_str()));
        assert!(
            !page.archive_read_ready,
            "the earlier native prefix remains unverified"
        );
        assert!(page.revision.starts_with("archive1:"));
        assert_eq!(
            page.conversation_id.as_deref(),
            Some(conversation_id.as_str())
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
            let unchanged = crate::commands::chat::load_agent_chat_page_with_preview_for_state(
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
            assert!(!unchanged.archive_read_ready && !unchanged.unchanged);
            assert_eq!(unchanged.revision, page.revision);
            assert_eq!(unchanged.events, page.events);
            assert_eq!(unchanged.progress, page.progress);
            assert_eq!(unchanged.generation, page.generation);
            assert_eq!(unchanged.source_epoch, page.source_epoch);
            assert_eq!(unchanged.next_before, page.next_before);
            assert!(unchanged.recent_admission.is_none());
            assert!(unchanged.revalidated_ids.is_none());
            let stale = crate::commands::chat::load_agent_chat_page_with_preview_for_state(
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
    let body = crate::commands::chat::load_agent_chat_page_with_preview_for_state(
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
    assert!(!body.archive_read_ready);
    assert_eq!(body.progress, "archive_history_unverified");
    assert_eq!(body.source_epoch.as_deref(), Some(expected_epoch.as_str()));
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
        "Normal delivered prompt after the final adoption chunk",
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
async fn archive_preview_excludes_unarchived_watch_history_when_logging_is_disabled() {
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
    let page = crate::commands::chat::load_agent_chat_page_with_preview_for_state(
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
    assert!(page.events.is_empty() && page.recent_admission.is_none());
    assert_eq!(page.progress, "archive_empty");
    assert!(state
        .conversation_archive
        .chat_events_for_agent(session)
        .unwrap()
        .is_empty());
    let conversations = wardian_core::paths::agent_conversations_dir(session).unwrap();
    assert!(!conversations.with_file_name("chat-read-objects").exists());
}

async fn missing_body_fixture(
    state: &AppState,
    home: &crate::control::test_support::TestWardianHome,
    body_index: usize,
    body_repetitions: usize,
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
    let body = "saved artifact body\n".repeat(body_repetitions);
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
    assert!(!conversations.with_file_name("chat-read-objects").exists());
    assert!(!conversations.with_file_name("chat-read-head.json").exists());
    (config, original, artifact, body, saved)
}

async fn missing_body_page(
    state: &AppState,
    session_id: &str,
    cursor: Option<String>,
) -> wardian_core::models::chat::AgentChatPage {
    crate::commands::chat::load_agent_chat_page_with_preview_for_state(
        state,
        session_id.into(),
        cursor,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead {
            limit: Some(24),
            preview: false,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn cold_archive_older_reads_owned_artifact_after_restart() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = AppState::new();
    let (config, original, _, body, saved) = missing_body_fixture(&state, &home, 133, 7000).await;
    let first = missing_body_page(&state, &config.session_id, None).await;
    assert_eq!(first.events.len(), 24);
    assert_eq!(first.conversation_id.as_deref(), Some(original.as_str()));

    let restarted = AppState::new();
    register_cold_scope_agent(&restarted, &config).await;
    let older = missing_body_page(&restarted, &config.session_id, first.next_before).await;
    assert!(older.archive_read_ready && older.older_page && !older.reset);
    assert_eq!(older.generation, first.generation);
    let row = older
        .events
        .iter()
        .find(|event| event.id == "missing-body-133")
        .unwrap();
    let mut detail = row.metadata["chat_detail_ref"].as_str().map(str::to_owned);
    let mut contents = String::new();
    while let Some(reference) = detail.take() {
        let page = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
            &restarted,
            config.session_id.clone(),
            None,
            None,
            Some(reference),
            None,
        )
        .await
        .unwrap();
        let body = page.detail.unwrap();
        contents.push_str(&body.text);
        detail = body.next;
    }
    assert_eq!(contents, body);
    for (path, bytes) in saved {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn cold_archive_missing_artifact_keeps_the_owned_header_and_older_frontier() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = AppState::new();
    let (config, original, artifact, _, saved) =
        missing_body_fixture(&state, &home, 133, 7000).await;
    std::fs::remove_file(&artifact).unwrap();
    let first = missing_body_page(&state, &config.session_id, None).await;
    assert_eq!(first.conversation_id.as_deref(), Some(original.as_str()));
    assert_eq!(first.progress, "archive_payload_missing");
    assert!(!first.archive_read_ready);
    let older = missing_body_page(&state, &config.session_id, first.next_before).await;
    assert!(older.older_page && !older.reset);
    assert!(older.next_before.is_some());
    let row = older
        .events
        .iter()
        .find(|event| event.id == "missing-body-133")
        .unwrap();
    assert_eq!(row.metadata["chat_body_unavailable"], true);
    assert!(row.metadata["chat_detail_ref"].is_null());
    assert!(!artifact.exists());
    for (path, bytes) in saved {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn cold_archive_removed_body_invalidates_a_detail_reference_without_repair() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = AppState::new();
    let (config, _, artifact, body, saved) = missing_body_fixture(&state, &home, 160, 7000).await;
    let first = missing_body_page(&state, &config.session_id, None).await;
    let row = first
        .events
        .iter()
        .find(|event| event.id == "missing-body-160")
        .unwrap();
    let reference = row.metadata["chat_detail_ref"].as_str().unwrap().to_owned();
    let detail = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        config.session_id.clone(),
        None,
        None,
        Some(reference),
        None,
    )
    .await
    .unwrap()
    .detail
    .unwrap();
    assert!(body.starts_with(&detail.text) && !detail.complete);
    let continuation = detail.next.unwrap();
    std::fs::remove_file(&artifact).unwrap();
    let unavailable = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        config.session_id.clone(),
        None,
        None,
        Some(continuation),
        None,
    )
    .await
    .unwrap();
    assert!(unavailable.reset && unavailable.detail.is_none());
    assert_eq!(unavailable.progress, "archive_detail_unavailable");
    let older = missing_body_page(&state, &config.session_id, first.next_before).await;
    assert!(older.older_page && !older.events.is_empty());
    for (path, bytes) in saved {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn cold_archive_nonregular_artifact_fails_closed() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = AppState::new();
    let (config, _, artifact, _, saved) = missing_body_fixture(&state, &home, 160, 7000).await;
    std::fs::remove_file(&artifact).unwrap();
    std::fs::create_dir(&artifact).unwrap();
    let result = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
        &state,
        config.session_id.clone(),
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(result.is_err(), "a directory cannot supply the owned body");
    for (path, bytes) in saved {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

async fn cold_scope_archive_fixture(
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
        config.fresh_provider_session_id = Some("scope-primary".into());
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
    let (config, original) = cold_scope_archive_fixture(&state, &home, with_sessions).await;
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
    if with_sessions {
        let restored = AppState::new();
        register_cold_scope_agent(&restored, &result.config).await;
        for state in [state.inner(), &restored] {
            let read = crate::commands::chat::load_agent_chat_page_with_revalidation_for_state(
                state,
                config.session_id.clone(),
                None,
                Some(first.revision.clone()),
                Some(old_detail.clone()),
                None,
            )
            .await
            .unwrap_err();
            assert_eq!(read, "chat archive source identity conflicts");
        }
        return;
    }
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
