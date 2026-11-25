use super::*;
use crate::control::test_support::TestWardianHome;
use wardian_core::conversations::{
    write_jsonl_atomic, ConversationBoundaryReason, ConversationTurnRecord,
};

const ID: &str = "archive-only-agent";

async fn registered(logging: AgentConversationLoggingSetting) -> AppState {
    let state = AppState::new();
    let agent = crate::state::ActiveAgent {
        config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
            session_id: ID.into(),
            provider: "mock".into(),
            conversation_logging: logging,
            ..Default::default()
        })),
        child_process: None,
        background_processes: Vec::new(),
        memory_capability: None,
        runtime_generation: None,
        process_id: None,
        query_count: Default::default(),
        init_timestamp: Default::default(),
        last_query_timestamp: Default::default(),
        current_status: Arc::new(Mutex::new("Off".into())),
        last_status_at: Default::default(),
        watch_state: Arc::new(Mutex::new(AgentWatchState::new(ID.into(), 16, 1024))),
        terminal_title: Default::default(),
        last_output_at: Default::default(),
        log_path: Default::default(),
        log_last_modified: Default::default(),
        #[cfg(windows)]
        job_object: None,
    };
    state.agents.lock().await.insert(ID.into(), agent);
    state
}

async fn read(
    state: &AppState,
    before: Option<String>,
    detail: Option<String>,
    preview: bool,
) -> wardian_core::models::chat::AgentChatPage {
    load_agent_chat_page_with_preview_for_state(
        state,
        ID.into(),
        before,
        None,
        detail,
        None,
        RecentChatRead {
            limit: Some(2),
            preview,
        },
    )
    .await
    .unwrap()
}

fn archive_snapshot(path: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn visit(path: &Path, contents: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(&entry.path(), contents);
            } else {
                contents.insert(entry.path(), std::fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut contents = std::collections::BTreeMap::new();
    visit(path, &mut contents);
    contents
}

#[tokio::test]
async fn archive_only_chat_entry_reads_recent_older_preview_and_artifact_without_writes() {
    let _home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let context = ConversationArchiveContext::for_agent_id(ID, "mock");
    let body = "長い本文".repeat(14_000);
    for text in ["historical one", body.as_str(), "latest three"] {
        state
            .conversation_archive
            .append_delivered_input_with_context(context.clone(), text, None)
            .unwrap();
    }
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let objects = root.parent().unwrap().join("chat-read-objects");
    assert!(!objects.exists());
    let expected = archive_snapshot(&root);
    let recent = read(&state, None, None, false).await;
    assert!(recent.archive_read_ready);
    assert_eq!(
        recent.events.last().unwrap().text.as_deref(),
        Some("latest three")
    );
    let preview = crate::remote::operations::remote_agent_chat_page_with_preview(
        &state,
        ID,
        None,
        None,
        None,
        None,
        RecentChatRead {
            limit: Some(2),
            preview: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(preview.events, recent.events);
    assert_eq!(preview.revision, recent.revision);
    assert!(preview.records_decoded < recent.records_decoded);
    assert!(preview.records_decoded <= 6);
    let older = read(&state, recent.next_before.clone(), None, false).await;
    assert!(older.older_page && !older.reset);
    assert_eq!(older.events[0].text.as_deref(), Some("historical one"));
    let mut continuation = recent.events[0].metadata["chat_detail_ref"]
        .as_str()
        .map(str::to_owned);
    assert!(continuation.is_some(), "owned artifact must have detail");
    let mut recovered = String::new();
    while let Some(reference) = continuation {
        let detail = read(&state, None, Some(reference), false)
            .await
            .detail
            .unwrap();
        recovered.push_str(&detail.text);
        continuation = detail.next;
    }
    assert_eq!(recovered, body);
    let restarted = registered(AgentConversationLoggingSetting::Disabled).await;
    let disabled = read(&restarted, None, None, true).await;
    assert_eq!(disabled.progress, "logging_disabled");
    assert_eq!(disabled.events, recent.events);
    let old = read(
        &restarted,
        Some("legacy-head:00000000000000000001".into()),
        None,
        false,
    )
    .await;
    assert!(old.reset && !old.older_page && old.archive_read_ready);
    assert_eq!(archive_snapshot(&root), expected);
    assert!(!objects.exists());
}

#[tokio::test]
async fn archive_only_chat_entry_proves_append_torn_tail_turn_replacement_and_restart() {
    use std::io::Write;

    let _home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let context = ConversationArchiveContext::for_agent_id(ID, "mock");
    for text in ["one", "two", "three"] {
        state
            .conversation_archive
            .append_delivered_input_with_context(context.clone(), text, None)
            .unwrap();
    }
    let before = read(&state, None, None, false).await;
    let older_cursor = before.next_before.clone();
    let directory =
        wardian_core::paths::agent_conversation_dir(ID, before.conversation_id.as_deref().unwrap())
            .unwrap();
    let turn_path = directory.join("turns.jsonl");
    let mut turns: Vec<ConversationTurnRecord> =
        wardian_core::conversations::read_jsonl_records(&turn_path).unwrap();
    turns.last_mut().unwrap().turn_key = "atomic-replacement".into();
    write_jsonl_atomic(&turn_path, &turns).unwrap();
    let changed = read(&state, None, None, false).await;
    assert_ne!(changed.revision, before.revision);
    state
        .conversation_archive
        .append_delivered_input_with_context(context, "four", None)
        .unwrap();
    let appended = read(&state, None, None, false).await;
    assert_eq!(
        appended.events.last().unwrap().text.as_deref(),
        Some("four")
    );
    let path = directory.join("events.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(br#"{"text":"torn"#)
        .unwrap();
    let torn = read(&state, None, None, false).await;
    assert_eq!(torn.progress, "archive_tail_pending");
    assert_eq!(torn.events, appended.events);
    let restarted = registered(AgentConversationLoggingSetting::Enabled).await;
    let older = read(&restarted, older_cursor, None, false).await;
    assert!(older.older_page && !older.reset);
    assert_eq!(older.events[0].text.as_deref(), Some("one"));
    assert!(!directory
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("chat-read-objects")
        .exists());
}

#[test]
fn archive_only_chat_entry_dispatches_desktop_recent_preview_older_and_detail() {
    let _home = TestWardianHome::new();
    let state =
        tauri::async_runtime::block_on(registered(AgentConversationLoggingSetting::Enabled));
    let context = ConversationArchiveContext::for_agent_id(ID, "mock");
    let body = "owned detail ".repeat(12_000);
    for text in ["older retained", body.as_str(), "recent retained"] {
        state
            .conversation_archive
            .append_delivered_input_with_context(context.clone(), text, None)
            .unwrap();
    }
    let limited = tauri::async_runtime::block_on(read(&state, None, None, false));
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let expected = archive_snapshot(&root);
    let app = tauri::test::mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![load_agent_chat_page])
        .build(crate::app_context())
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let invoke = |params| {
        tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "load_agent_chat_page".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "https://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: tauri::ipc::InvokeBody::Json(params),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .unwrap()
        .deserialize::<AgentChatPage>()
        .unwrap()
    };
    use serde_json::json;
    let recent = invoke(json!({ "sessionId": ID }));
    let preview = invoke(json!({ "sessionId": ID, "recentPreview": true }));
    assert_eq!(recent.events, preview.events);
    assert!(recent.archive_read_ready);
    let older = invoke(json!({
        "sessionId": ID, "cursor": limited.next_before.unwrap(), "revision": limited.revision
    }));
    assert!(older.older_page && !older.reset);
    assert_eq!(older.events[0].text.as_deref(), Some("older retained"));
    let detail = invoke(json!({
        "sessionId": ID, "detailRef": recent.events[1].metadata["chat_detail_ref"]
    }));
    assert!(body.starts_with(&detail.detail.unwrap().text));
    assert_eq!(archive_snapshot(&root), expected);
    assert!(!root.parent().unwrap().join("chat-read-objects").exists());
}

#[tokio::test]
async fn archive_only_chat_entry_excludes_foreign_scope_and_reports_missing_payload() {
    use wardian_core::conversations::{read_jsonl_records, ConversationNarrativeRecord};

    let _home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let own = ConversationArchiveContext::for_agent_id(ID, "mock");
    let body = "original artifact ".repeat(12_000);
    for text in ["earlier authorized", body.as_str(), "recent authorized"] {
        state
            .conversation_archive
            .append_delivered_input_with_context(own.clone(), text, None)
            .unwrap();
    }
    let initial = read(&state, None, None, false).await;
    let older_cursor = initial.next_before.clone().unwrap();
    let detail_ref = initial.events[0].metadata["chat_detail_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let own_dir = root.join(initial.conversation_id.unwrap());
    state
        .conversation_archive
        .rollover_agent(ID, ConversationBoundaryReason::ProviderSourceChanged)
        .unwrap();
    let mut foreign = own;
    foreign.provider_session_ids = vec!["foreign-session".into()];
    foreign.provider_source_key = Some("mock:session:foreign-session".into());
    state
        .conversation_archive
        .append_delivered_input_with_context(foreign, "foreign payload", None)
        .unwrap();
    let closed = read(&state, None, None, false).await;
    assert_eq!(closed.events, initial.events);
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&own_dir.join("conversation.jsonl")).unwrap();
    let reference = &records[1].artifact_refs[0];
    let artifact = if Path::new(reference).components().count() == 1 {
        own_dir.join("artifacts").join(reference)
    } else {
        own_dir.join(reference)
    };
    let held = artifact.with_extension("held-test-fixture");
    std::fs::rename(&artifact, &held).unwrap();
    let before_read = archive_snapshot(&root);
    let missing = read(&state, None, None, false).await;
    assert_eq!(missing.progress, "archive_payload_missing");
    assert!(!missing.archive_read_ready);
    assert_eq!(missing.events.len(), 2);
    assert_eq!(missing.events[0].metadata["chat_body_unavailable"], true);
    let detail = read(&state, None, Some(detail_ref), false).await;
    assert!(detail.reset && detail.detail.is_none());
    assert_eq!(detail.progress, "archive_detail_unavailable");
    let older = read(&state, Some(older_cursor.clone()), None, false).await;
    assert!(older.older_page && !older.reset);
    assert_eq!(older.events[0].text.as_deref(), Some("earlier authorized"));
    assert_eq!(archive_snapshot(&root), before_read);
    std::fs::rename(&held, &artifact).unwrap();

    let events_path = own_dir.join("events.jsonl");
    let mut events: Vec<AgentChatEvent> = read_jsonl_records(&events_path).unwrap();
    // The same logical ID cannot rebind a cursor to different frame bytes.
    events[1].metadata["fixture_replacement"] = serde_json::json!(true);
    write_jsonl_atomic(&events_path, &events).unwrap();
    let reset = read(&state, Some(older_cursor), None, false).await;
    assert!(reset.reset && !reset.older_page);
    assert!(reset.archive_read_ready);
    assert!(!root.parent().unwrap().join("chat-read-objects").exists());
}

#[tokio::test]
async fn archive_only_chat_entry_captures_framed_codex_rows_and_receipt_without_projection() {
    use serde_json::json;
    use std::io::Write;

    let test_home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let native = "00000000-0000-4000-8000-000000000145";
    let path = test_home
        .path()
        .join(format!("rollout-2026-10-04T00-00-00-{native}.jsonl"));
    let header = json!({"type":"session_meta", "payload":{"id":native,"cwd":test_home.path()}});
    let request = json!({
        "type":"event_msg", "timestamp":"2026-10-04T12:00:00Z",
        "payload":{"type":"user_message","message":"framed native prompt"}
    });
    std::fs::write(&path, format!("{header}\n{request}\n")).unwrap();
    {
        let roster = state.agents.lock().await;
        let agent = &roster[ID];
        let mut config = agent.config.lock().unwrap();
        config.provider = "codex".into();
        // Admit the initial prefix through the real fresh-source rule. A
        // legacy resume with no checkpoint must not import old native bytes.
        config.fresh_provider_session_id = Some(native.into());
        *agent.log_path.lock().unwrap() = Some(path.clone());
    }
    let empty = read(&state, None, None, true).await;
    assert_eq!(empty.progress, "archive_empty");
    let observe_and_capture = || async {
        let request = background_capture_request(&state, ID)
            .await
            .unwrap()
            .unwrap();
        assert!(request.source.is_some());
        let claim =
            crate::commands::background_capture::admit_background_capture(&state, request, false)
                .await
                .expect("ordinary capture admission");
        crate::commands::background_capture::run_background_capture(&state, claim).await;
    };
    observe_and_capture().await;
    let captured = read(&state, None, None, false).await;
    assert!(captured.archive_read_ready, "{}", captured.progress);
    assert!(captured.source_epoch.is_some());
    assert_eq!(
        captured.events.last().unwrap().text.as_deref(),
        Some("framed native prompt")
    );
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let agent_dir = root.parent().unwrap();
    let response = json!({
        "type":"event_msg", "timestamp":"2026-10-04T12:00:01Z",
        "payload":{"type":"agent_message","message":"framed native response"}
    });
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
        "{response}"
    )
    .unwrap();
    observe_and_capture().await;
    let (snapshot, _) = chat_read_snapshot_for_state(&state, ID).unwrap();
    let context = conversation_archive_context_from_snapshot(&snapshot);
    let fence = state
        .conversation_archive
        .archive_input_fence(&context, Some(&path))
        .unwrap()
        .unwrap();
    let receipt = state
        .conversation_archive
        .append_delivered_input_receipt(context, "archive-only receipt", &fence)
        .unwrap()
        .unwrap();
    let retained = archive_snapshot(&root);
    let saved_log = path.with_extension("held-test-fixture");
    std::fs::rename(&path, saved_log).unwrap();
    let recent = read(&state, None, None, true).await;
    assert_eq!(recent.events.last().unwrap().id, receipt.chat_event_id);
    assert!(recent
        .events
        .iter()
        .any(|row| row.text.as_deref() == Some("framed native response")));
    state.agents.lock().await[ID]
        .config
        .lock()
        .unwrap()
        .conversation_logging = AgentConversationLoggingSetting::Disabled;
    let disabled = read(&state, None, None, true).await;
    assert_eq!(disabled.progress, "logging_disabled");
    assert_eq!(disabled.events, recent.events);
    assert_eq!(archive_snapshot(&root), retained);
    for legacy in [
        "chat-read-objects",
        "chat-read-head.json",
        "chat-source-head.json",
    ] {
        assert!(!agent_dir.join(legacy).exists(), "{legacy} was created");
    }
}
fn archived_event(
    provider: &str,
    id: &str,
    kind: AgentChatEventKind,
    role: Option<AgentChatRole>,
    text: Option<String>,
    metadata: serde_json::Value,
) -> AgentChatEvent {
    AgentChatEvent {
        id: id.into(),
        session_id: ID.into(),
        provider: provider.into(),
        kind,
        role,
        text,
        title: None,
        status: None,
        turn_id: None,
        source: None,
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: None,
        sequence: None,
        metadata,
    }
}

#[tokio::test]
async fn archive_only_chat_entry_uses_exact_writer_coalescence_receipt() {
    use serde_json::json;
    use wardian_core::conversations::{read_jsonl_records, ConversationNarrativeRecord};

    let _root = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let native_id = "00000000-0000-4000-8000-000000000146";
    state.agents.lock().await[ID]
        .config
        .lock()
        .unwrap()
        .fresh_provider_session_id = Some(native_id.into());
    let (snapshot, _) = chat_read_snapshot_for_state(&state, ID).unwrap();
    let context = conversation_archive_context_from_snapshot(&snapshot);
    state
        .conversation_archive
        .append_delivered_input_with_context(context.clone(), "broker input", None)
        .unwrap();
    let before = read(&state, None, None, false).await;
    let generated = before.events[0].id.clone();
    let native = archived_event(
        "mock",
        "provider-native-reconciled",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("broker input".into()),
        json!({"provider_log":true, "provider_session_id":native_id, "legacy_event_ids":[generated]}),
    );
    state
        .conversation_archive
        .append_chat_events_with_context(context, &[native])
        .unwrap();
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let dir = root.join(before.conversation_id.unwrap());
    let stored: Vec<AgentChatEvent> = read_jsonl_records(&dir.join("events.jsonl")).unwrap();
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&dir.join("conversation.jsonl")).unwrap();
    assert_eq!(stored.len(), 1);
    assert!(records[0]
        .event_refs
        .contains(&"provider-native-reconciled".to_owned()));
    assert_eq!(
        stored[0].metadata["archive_coalesced_inputs"][0]["removed_id"],
        "provider-native-reconciled"
    );
    let page = read(&state, None, None, false).await;
    assert!(page.archive_read_ready, "{}", page.progress);
    assert_eq!(page.events.len(), 1);
    let mut unproven = stored;
    unproven[0]
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("archive_coalesced_inputs");
    write_jsonl_atomic(&dir.join("events.jsonl"), &unproven).unwrap();
    let incomplete = read(&state, None, None, false).await;
    assert_eq!(incomplete.progress, "archive_payload_missing");
    assert_eq!(incomplete.events[0].id, generated);
    assert!(!root.parent().unwrap().join("chat-read-objects").exists());
}

#[tokio::test]
async fn archive_only_chat_entry_completion_requires_full_saved_payload_and_framing() {
    use std::io::Write;
    use wardian_core::conversations::{read_jsonl_records, ConversationNarrativeRecord};

    let _root = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let context = ConversationArchiveContext::for_agent_id(ID, "mock");
    let request = "owned full request ".repeat(12_000);
    let answer = "owned full answer 世界 ".repeat(13_000);
    state
        .conversation_archive
        .append_delivered_input_with_context(context.clone(), &request, None)
        .unwrap();
    state
        .conversation_archive
        .append_chat_events_with_context(
            context.clone(),
            &[archived_event(
                "mock",
                "saved-answer",
                AgentChatEventKind::Message,
                Some(AgentChatRole::Assistant),
                Some(answer.clone()),
                serde_json::json!({}),
            )],
        )
        .unwrap();
    let page = read(&state, None, None, false).await;
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let dir = root.join(page.conversation_id.unwrap());
    let restored = registered(AgentConversationLoggingSetting::Enabled).await;
    let saved = restored
        .conversation_archive
        .archive_completion_events(&context)
        .unwrap();
    assert!(
        saved[0].text.as_deref() == Some(request.as_str()),
        "saved request differs"
    );
    assert!(
        saved[1].text.as_deref() == Some(answer.as_str()),
        "saved answer differs"
    );
    let exact = observe_current_turn_transcript(&restored, ID)
        .await
        .unwrap();
    assert!(
        exact[0].text.as_deref() == Some(request.trim()),
        "completion request differs"
    );
    assert!(
        exact[1].text.as_deref() == Some(answer.trim()),
        "completion answer differs"
    );
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&dir.join("conversation.jsonl")).unwrap();
    let artifact = dir.join("artifacts").join(&records[1].artifact_refs[0]);
    let original = std::fs::read(&artifact).unwrap();
    let mut changed = original.clone();
    changed[0] = b'x';
    std::fs::write(&artifact, &changed).unwrap();
    assert!(
        observe_current_turn_transcript(&restored, ID)
            .await
            .is_err(),
        "same-length replacement must not supply completion text"
    );
    std::fs::write(&artifact, &original).unwrap();
    let held = artifact.with_extension("held");
    std::fs::rename(&artifact, &held).unwrap();
    assert!(observe_current_turn_transcript(&restored, ID)
        .await
        .is_err());
    std::fs::rename(held, artifact).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("events.jsonl"))
        .unwrap()
        .write_all(b"{\"torn\":")
        .unwrap();
    assert!(observe_current_turn_transcript(&restored, ID)
        .await
        .is_err());
    assert!(!root.parent().unwrap().join("chat-read-objects").exists());
}

#[tokio::test]
async fn archive_only_chat_entry_artifact_readiness_revision_and_empty_catalog_agree() {
    use wardian_core::conversations::{read_jsonl_records, ConversationNarrativeRecord};

    let _root = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let context = ConversationArchiveContext::for_agent_id(ID, "mock");
    let body = "old archive artifact ".repeat(12_000);
    for text in [body.as_str(), "more recent", "most recent"] {
        state
            .conversation_archive
            .append_delivered_input_with_context(context.clone(), text, None)
            .unwrap();
    }
    let regular = read(&state, None, None, false).await;
    assert!(regular.archive_read_ready);
    let older = read(&state, regular.next_before.clone(), None, false).await;
    assert!(older.older_page);
    assert_eq!(older.revision, regular.revision);
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let dir = root.join(regular.conversation_id.as_ref().unwrap());
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&dir.join("conversation.jsonl")).unwrap();
    let path = dir.join("artifacts").join(&records[0].artifact_refs[0]);
    let held = path.with_extension("held");
    std::fs::rename(&path, &held).unwrap();
    let missing = read(&state, None, None, false).await;
    assert_eq!(missing.events, regular.events);
    assert_eq!(missing.progress, "archive_payload_missing");
    assert!(!missing.archive_read_ready);
    assert_ne!(missing.revision, regular.revision);
    std::fs::rename(held, path).unwrap();
    let recovered = read(&state, None, None, false).await;
    assert_eq!(recovered.revision, regular.revision);
    let catalog = root.join("index.jsonl");
    std::fs::rename(&catalog, catalog.with_extension("held")).unwrap();
    let empty = read(&state, None, None, false).await;
    assert_eq!(empty.progress, "archive_catalog_unavailable");
    assert_eq!(empty.revision, "archive1:archive_catalog_unavailable");
    assert!(
        empty.conversation_id.is_none() && empty.events.is_empty() && empty.next_before.is_none()
    );
    assert!(!root.parent().unwrap().join("chat-read-objects").exists());
}

#[tokio::test]
async fn archive_only_chat_entry_typed_turn_step_and_compound_artifact_body() {
    use serde_json::json;
    let _root = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    state.agents.lock().await[ID]
        .config
        .lock()
        .unwrap()
        .provider = "claude".into();
    let context = ConversationArchiveContext::for_agent_id(ID, "claude");
    let input = "fixture full tool arguments";
    let output = "fixture output 世界 ".repeat(14_000);
    let request = archived_event(
        "claude",
        "request",
        AgentChatEventKind::Message,
        Some(AgentChatRole::User),
        Some("run a synthetic tool".into()),
        json!({"input_origin":"human_input", "input_purpose":"request", "request_root_id":"root"}),
    );
    let call = archived_event(
        "claude",
        "invocation",
        AgentChatEventKind::ToolCall,
        Some(AgentChatRole::Assistant),
        None,
        json!({"tool_name":"fixture-tool", "tool_call_id":"call-1", "request_root_id":"root", "tool_input_text":input}),
    );
    let result = archived_event(
        "claude",
        "tool-output",
        AgentChatEventKind::ToolResult,
        None,
        Some(output.clone()),
        json!({"tool_name":"fixture-tool", "tool_call_id":"call-1", "request_root_id":"root", "tool_input_text":input}),
    );
    let answer = archived_event(
        "claude",
        "answer",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("done".into()),
        json!({"request_root_id":"root", "provider_phase":"final_answer"}),
    );
    state
        .conversation_archive
        .append_chat_events_with_context(context, &[request, call, result, answer])
        .unwrap();
    let page = load_agent_chat_page_with_preview_for_state(
        &state,
        ID.into(),
        None,
        None,
        None,
        None,
        RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert!(page.archive_read_ready);
    let view = page
        .presentation
        .as_ref()
        .expect("archive typed turn")
        .decode_v1()
        .unwrap()
        .unwrap();
    let args = page
        .events
        .iter()
        .find(|event| event.id == "invocation")
        .unwrap();
    let short = args.metadata["chat_detail_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        read(&state, None, Some(short), false)
            .await
            .detail
            .unwrap()
            .text,
        input
    );
    assert_eq!(view.turns.len(), 1);
    let step = view
        .items
        .iter()
        .find_map(|item| item.step.as_ref())
        .unwrap();
    let detail = read(&state, None, step.detail_ref.clone(), false).await;
    assert_eq!(detail.revision, page.revision);
    let typed = detail
        .presentation
        .unwrap()
        .decode_v1()
        .unwrap()
        .unwrap()
        .detail
        .unwrap();
    assert_eq!(typed.input_text.as_deref(), Some(input));
    assert!(typed.truncated_before && output.ends_with(&typed.text));
    assert_eq!(typed.total_bytes, Some(output.len() as u64));
    let mut next = page
        .events
        .iter()
        .find(|row| row.id == "tool-output")
        .unwrap()
        .metadata["chat_detail_ref"]
        .as_str()
        .map(str::to_owned);
    assert!(next.is_some());
    let mut compound = String::new();
    while let Some(reference) = next {
        let detail = read(&state, None, Some(reference), false)
            .await
            .detail
            .unwrap();
        compound.push_str(&detail.text);
        next = detail.next;
    }
    assert_eq!(compound, format!("{input}\n\n{output}"));
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    assert!(!root.parent().unwrap().join("chat-read-objects").exists());
}

#[tokio::test]
async fn archive_only_chat_entry_reconciles_same_owned_native_refresh() {
    use serde_json::json;
    use wardian_core::conversations::{read_jsonl_records, ConversationNarrativeRecord};

    let _home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let native = "00000000-0000-4000-8000-000000000147";
    state.agents.lock().await[ID]
        .config
        .lock()
        .unwrap()
        .fresh_provider_session_id = Some(native.into());
    let (snapshot, _) = chat_read_snapshot_for_state(&state, ID).unwrap();
    let context = conversation_archive_context_from_snapshot(&snapshot);
    let event = archived_event(
        "mock",
        "native-owner",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("one observation".into()),
        json!({"provider_log":true, "provider_session_id":native}),
    );
    state
        .conversation_archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&event))
        .unwrap();
    let before = read(&state, None, None, false).await;
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let dir = root.join(before.conversation_id.unwrap());
    let event_path = dir.join("events.jsonl");
    let narrative_path = dir.join("conversation.jsonl");
    let mut existing: Vec<AgentChatEvent> = read_jsonl_records(&event_path).unwrap();
    let mut duplicate = existing[0].clone();
    duplicate.id = "distinct-native-frame".into();
    duplicate.metadata["legacy_event_ids"] = json!([event.id]);
    existing.push(duplicate);
    let mut narrative: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&narrative_path).unwrap();
    narrative[0].event_refs.push("distinct-native-frame".into());
    write_jsonl_atomic(&event_path, &existing).unwrap();
    write_jsonl_atomic(&narrative_path, &narrative).unwrap();

    state
        .conversation_archive
        .append_chat_events_with_context(context, &[event])
        .unwrap();
    let stored: Vec<AgentChatEvent> = read_jsonl_records(&event_path).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].metadata["archive_refreshed_observations"][0]["removed_id"],
        "distinct-native-frame"
    );
    let page = read(&state, None, None, false).await;
    assert!(page.archive_read_ready, "{}", page.progress);
    assert_eq!(page.events.len(), 1);
    assert!(!root.parent().unwrap().join("chat-read-objects").exists());
}

#[tokio::test]
async fn archive_only_chat_entry_uses_nested_artifact_when_top_level_is_null() {
    use wardian_core::conversations::read_jsonl_records;
    let _home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let text = "owned nested artifact 世界 ".repeat(10_000);
    let context = ConversationArchiveContext::for_agent_id(ID, "mock");
    state
        .conversation_archive
        .append_delivered_input_with_context(context, &text, None)
        .unwrap();
    let before = read(&state, None, None, false).await;
    let root = wardian_core::paths::agent_conversations_dir(ID).unwrap();
    let path = root
        .join(before.conversation_id.unwrap())
        .join("events.jsonl");
    let mut events: Vec<AgentChatEvent> = read_jsonl_records(&path).unwrap();
    events[0].metadata["text_artifact_refs"] = serde_json::Value::Null;
    write_jsonl_atomic(&path, &events).unwrap();
    let recent = read(&state, None, None, false).await;
    let detail = recent.events[0].metadata["chat_detail_ref"]
        .as_str()
        .expect("nested owned detail");
    let body = read(&state, None, Some(detail.into()), false)
        .await
        .detail
        .unwrap();
    assert!(text.starts_with(&body.text) && !body.complete && !body.text.is_empty());
}

#[tokio::test]
async fn archive_only_chat_entry_defers_completion_before_more_capture() {
    use serde_json::json;
    let home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let native = "00000000-0000-4000-8000-000000000148";
    let path = home
        .path()
        .join(format!("rollout-2026-10-04T00-00-00-{native}.jsonl"));
    let header = json!({"type":"session_meta","payload":{"id":native}});
    let request =
        json!({"type":"event_msg", "payload":{"type":"user_message","message":"request"}});
    let first = json!({"type":"event_msg", "payload":{"type":"agent_message","message":"a".repeat(270_000)}});
    let final_answer = json!({"type":"event_msg", "payload":{"type":"agent_message","message":"final captured answer"}});
    std::fs::write(
        &path,
        format!("{header}\n{request}\n{first}\n{final_answer}\n"),
    )
    .unwrap();
    {
        let roster = state.agents.lock().await;
        let agent = &roster[ID];
        let mut config = agent.config.lock().unwrap();
        config.provider = "codex".into();
        config.fresh_provider_session_id = Some(native.into());
        *agent.log_path.lock().unwrap() = Some(path);
    }
    assert_eq!(
        observe_current_turn_transcript(&state, ID)
            .await
            .unwrap_err(),
        "chat completion capture has more work"
    );
    let mut settled = None;
    for _ in 0..4 {
        match observe_current_turn_transcript(&state, ID).await {
            Ok(events) => {
                settled = Some(events);
                break;
            }
            Err(error) if error == "chat completion capture has more work" => continue,
            Err(error) => panic!("unexpected completion error while draining fixture: {error}"),
        }
    }
    let settled = settled.expect("bounded fixture should reach the final capture pass");
    assert!(settled
        .iter()
        .any(|row| row.text.as_deref() == Some("final captured answer")));
}

#[tokio::test]
async fn archive_only_chat_entry_legacy_completion_requires_exact_live_frame_and_bytes() {
    use serde_json::json;
    use wardian_core::conversations::{read_jsonl_records, ConversationNarrativeRecord};

    let home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let native = "00000000-0000-4000-8000-000000000149";
    let path = home
        .path()
        .join(format!("rollout-2026-10-04T00-00-00-{native}.jsonl"));
    let body = format!("{}end", "native artifact exact ".repeat(7_000));
    let header = json!({"type":"session_meta","payload":{"id":native}});
    let request = json!({"type":"event_msg","payload":{"type":"user_message","message":"request"}});
    let answer = json!({"type":"event_msg","payload":{"type":"agent_message","message":body}});
    std::fs::write(&path, format!("{header}\n{request}\n{answer}\n")).unwrap();
    {
        let roster = state.agents.lock().await;
        let agent = &roster[ID];
        let mut config = agent.config.lock().unwrap();
        config.provider = "codex".into();
        config.fresh_provider_session_id = Some(native.into());
        *agent.log_path.lock().unwrap() = Some(path);
    }
    let observed =
        archive_agent_chat_events_for_state_with_stage(&state, ID, CaptureLane::Background)
            .await
            .map_err(|failure| failure.message)
            .unwrap();
    assert!(!observed.continue_immediately);
    let page = read(&state, None, None, false).await;
    let path = wardian_core::paths::agent_conversations_dir(ID)
        .unwrap()
        .join(page.conversation_id.unwrap())
        .join("conversation.jsonl");
    let mut records: Vec<ConversationNarrativeRecord> = read_jsonl_records(&path).unwrap();
    let row = records
        .iter_mut()
        .find(|record| record.text_artifact_proof.is_some())
        .unwrap();
    assert!(row.text_artifact_proof.take().is_some());
    write_jsonl_atomic(&path, &records).unwrap();

    assert!(state
        .conversation_archive
        .archive_completion_events_with_live(&observed.context, &[])
        .is_err());
    let exact = state
        .conversation_archive
        .archive_completion_events_with_live(&observed.context, &observed.events)
        .unwrap();
    assert!(exact
        .iter()
        .any(|event| event.text.as_deref() == Some(body.as_str())));
    let mut wrong = observed.events;
    let live = wrong
        .iter_mut()
        .find(|event| event.role == Some(AgentChatRole::Assistant))
        .unwrap();
    live.metadata["chat_source_start"] = json!(u64::MAX);
    assert!(state
        .conversation_archive
        .archive_completion_events_with_live(&observed.context, &wrong)
        .is_err());
}

#[tokio::test]
async fn archive_only_chat_entry_keeps_single_frame_refresh_alias_out_of_physical_refs() {
    use serde_json::json;
    use wardian_core::conversations::{read_jsonl_records, ConversationNarrativeRecord};

    let _home = TestWardianHome::new_async().await;
    let state = registered(AgentConversationLoggingSetting::Enabled).await;
    let native = "00000000-0000-4000-8000-000000000150";
    state.agents.lock().await[ID]
        .config
        .lock()
        .unwrap()
        .fresh_provider_session_id = Some(native.into());
    let (snapshot, _) = chat_read_snapshot_for_state(&state, ID).unwrap();
    let context = conversation_archive_context_from_snapshot(&snapshot);
    let first = archived_event(
        "mock",
        "native-a",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("bound archive observation".into()),
        json!({"provider_log":true, "provider_session_id":native}),
    );
    state
        .conversation_archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&first))
        .unwrap();
    let before = read(&state, None, None, false).await;
    assert!(before.archive_read_ready);
    let directory = wardian_core::paths::agent_conversations_dir(ID)
        .unwrap()
        .join(before.conversation_id.unwrap());
    let mut alias = first;
    alias.id = "native-b".into();
    alias.metadata["legacy_event_ids"] = json!(["native-a"]);
    state
        .conversation_archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&alias))
        .unwrap();

    let restarted = registered(AgentConversationLoggingSetting::Enabled).await;
    restarted.agents.lock().await[ID]
        .config
        .lock()
        .unwrap()
        .fresh_provider_session_id = Some(native.into());
    restarted
        .conversation_archive
        .append_chat_events_with_context(context, &[alias])
        .unwrap();
    let events: Vec<AgentChatEvent> = read_jsonl_records(&directory.join("events.jsonl")).unwrap();
    let records: Vec<ConversationNarrativeRecord> =
        read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "native-a");
    assert!(events[0].metadata["legacy_event_ids"]
        .as_array()
        .unwrap()
        .contains(&json!("native-b")));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event_refs, ["native-a"]);
    let after = read(&restarted, None, None, false).await;
    assert!(after.archive_read_ready, "{}", after.progress);
    assert_eq!(after.events.len(), 1);
    assert_eq!(after.events[0].id, "native-a");
}

async fn recover_artifact_after_event_append(proof_case: &str) {
    use serde_json::json;
    use wardian_core::conversations::{
        read_jsonl_records, ConversationNarrativeRecord, ConversationTextArtifactProof,
    };

    let _home = TestWardianHome::new_async().await;
    let writer = registered(AgentConversationLoggingSetting::Enabled).await;
    let context = ConversationArchiveContext::for_agent_id(ID, "mock");
    writer
        .conversation_archive
        .append_delivered_input_with_context(context.clone(), "recovery request", None)
        .unwrap();
    let initial = read(&writer, None, None, false).await;
    let directory = wardian_core::paths::agent_conversations_dir(ID)
        .unwrap()
        .join(initial.conversation_id.unwrap());
    let body = format!(
        "{}end",
        "exact body from event-first write 世界 ".repeat(10_000)
    );
    let response = archived_event(
        "mock",
        "event-first-answer",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some(body.clone()),
        json!({}),
    );
    writer
        .conversation_archive
        .fail_after_chat_event_append_for_test();
    let error = writer
        .conversation_archive
        .append_chat_events_with_context(context.clone(), std::slice::from_ref(&response))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "injected interruption after chat event append"
    );
    drop(writer);

    let events_path = directory.join("events.jsonl");
    let records_path = directory.join("conversation.jsonl");
    let mut events: Vec<AgentChatEvent> = read_jsonl_records(&events_path).unwrap();
    let previous: Vec<ConversationNarrativeRecord> = read_jsonl_records(&records_path).unwrap();
    assert_eq!(previous.len(), 1);
    let frame = events
        .iter_mut()
        .find(|event| event.id == "event-first-answer")
        .unwrap();
    assert!(frame.text.is_none());
    let proof: ConversationTextArtifactProof =
        serde_json::from_value(frame.metadata["text_artifact_proof"].clone()).unwrap();
    assert_eq!(
        frame.metadata["text_artifact_refs"],
        json!([proof.reference])
    );
    assert_eq!(proof.bytes, body.len() as u64);
    assert_eq!(
        proof.sha256,
        format!("{:x}", sha2::Sha256::digest(body.as_bytes()))
    );
    let artifact = directory.join("artifacts").join(&proof.reference);
    assert!(std::fs::read(&artifact).unwrap() == body.as_bytes());

    match proof_case {
        "exact" => {}
        "legacy" => {
            frame
                .metadata
                .as_object_mut()
                .unwrap()
                .remove("text_artifact_proof");
        }
        "foreign" => {
            frame.metadata["text_artifact_proof"]["reference"] = json!("unowned-reference.txt")
        }
        _ => panic!("invalid fixture"),
    }
    if proof_case != "exact" {
        write_jsonl_atomic(&events_path, &events).unwrap();
    }

    let restarted = registered(AgentConversationLoggingSetting::Enabled).await;
    // Advance the ordinary archive writer with a distinct observation. No
    // replay of the answer supplies its bytes or creates fresh proof.
    let boundary = archived_event(
        "mock",
        "after-recovery",
        AgentChatEventKind::Message,
        Some(AgentChatRole::Assistant),
        Some("later observation".into()),
        json!({}),
    );
    restarted
        .conversation_archive
        .append_chat_events_with_context(context.clone(), &[boundary])
        .unwrap();
    let records: Vec<ConversationNarrativeRecord> = read_jsonl_records(&records_path).unwrap();
    let recovered = records
        .iter()
        .find(|record| {
            record
                .event_refs
                .iter()
                .any(|id| id == "event-first-answer")
        })
        .unwrap();
    assert_eq!(
        recovered.artifact_refs,
        std::slice::from_ref(&proof.reference)
    );
    assert!(recovered.text.is_none());
    assert!(std::fs::read(&artifact).unwrap() == body.as_bytes());
    let result = restarted
        .conversation_archive
        .archive_completion_events(&context);
    if proof_case == "exact" {
        assert_eq!(recovered.text_artifact_proof.as_ref(), Some(&proof));
        assert!(result
            .unwrap()
            .iter()
            .any(|event| event.id == "event-first-answer"
                && event.text.as_deref() == Some(body.as_str())));
    } else {
        assert!(recovered.text_artifact_proof.is_none());
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn archive_only_chat_entry_restores_owned_artifact_proof_after_event_append() {
    recover_artifact_after_event_append("exact").await;
}

#[tokio::test]
async fn archive_only_chat_entry_recovery_refuses_legacy_or_foreign_artifact_proof() {
    recover_artifact_after_event_append("legacy").await;
    recover_artifact_after_event_append("foreign").await;
}
