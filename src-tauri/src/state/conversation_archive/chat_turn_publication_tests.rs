use super::tests::{candidate as base_candidate, event as base_event, snapshot};
use super::*;
use crate::state::conversation_archive::tests::isolated_home;
use crate::state::ConversationArchiveState;
use serde_json::Value;
use std::io::Write;
use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole, AgentChatStatus};
use wardian_core::models::chat_presentation::*;

fn context() -> ConversationArchiveContext {
    let mut context = ConversationArchiveContext::for_agent_id("agent-1", "codex");
    context.provider_source_key = Some("codex:synthetic".into());
    context
}

fn event(id: &str, text: &str) -> AgentChatEvent {
    let mut event = base_event(id, text);
    event.provider = "codex".into();
    event
}

fn candidate(events: Vec<AgentChatEvent>) -> Candidate {
    Candidate {
        context: context(),
        ..base_candidate(events)
    }
}

fn turn_events(output: Option<&str>) -> Vec<AgentChatEvent> {
    let mut prompt = event("synthetic-prompt", "Run the synthetic tests.");
    prompt.sequence = Some(1);
    prompt.metadata = json!({"request_root_id":"synthetic-request", "input_origin":"human_input", "input_purpose":"request"});
    let mut call = event("synthetic-call", "");
    call.sequence = Some(2);
    call.kind = AgentChatEventKind::ToolCall;
    call.role = None;
    call.turn_id = Some("synthetic-call-id".into());
    call.status = Some(AgentChatStatus::Running);
    call.metadata = json!({"request_root_id":"synthetic-request", "tool_name":"exec_command", "tool_input": {"description":"Run synthetic tests", "cmd":"echo synthetic", "context":"x".repeat(4000)}});
    let mut events = vec![prompt, call];
    if let Some(output) = output {
        let mut result = event("synthetic-result", output);
        result.sequence = Some(3);
        result.kind = AgentChatEventKind::ToolResult;
        result.role = Some(AgentChatRole::Tool);
        result.turn_id = Some("synthetic-call-id".into());
        result.status = Some(AgentChatStatus::Succeeded);
        result.metadata = json!({"request_root_id":"synthetic-request"});
        let mut answer = event("synthetic-answer", "The synthetic tests passed.");
        answer.sequence = Some(4);
        answer.role = Some(AgentChatRole::Assistant);
        answer.status = Some(AgentChatStatus::Succeeded);
        answer.metadata =
            json!({"request_root_id":"synthetic-request", "provider_phase":"final_answer"});
        let mut terminal = event("synthetic-terminal", "{\"machine_status\":\"complete\"}");
        terminal.sequence = Some(5);
        terminal.kind = AgentChatEventKind::Status;
        terminal.status = Some(AgentChatStatus::Succeeded);
        terminal.metadata = json!({"request_root_id":"synthetic-request", "raw_type":"result"});
        events.extend([result, answer, terminal]);
    }
    for event in &mut events {
        event.metadata["chat_turn_adapter"] = json!("codex_bracket_v1");
        event.metadata["chat_turn_membership_complete"] =
            json!(event.kind == AgentChatEventKind::Status);
    }
    events
}

fn settle(owner: &ProjectionOwner) {
    for _ in 0..256 {
        if !owner.advance("agent-1").unwrap() {
            return;
        }
    }
    panic!("synthetic turn did not settle in bounded checkpoints");
}

fn read_turn(detail: Option<&str>) -> AgentChatPage {
    read(&context(), &snapshot(&context()), None, None, detail).unwrap()
}

fn value(page: &AgentChatPage) -> ChatPresentationV1 {
    page.presentation
        .as_ref()
        .unwrap_or_else(|| panic!("canonical capability missing: progress={}, events={}, conversation={:?}, generation={:?}, epoch={:?}, recent={:?}, provisional_rows={}",
            page.progress, page.events.len(), page.conversation_id, page.generation, page.source_epoch, page.recent_admission,
            page.events.iter().filter(|event| event.metadata["chat_provisional"] == true || event.metadata["chat_identity_resolution"] == "unresolved").count()))
        .decode_v1()
        .unwrap()
        .unwrap()
}

// The public response omits the owner's staging certificate. These negative
// tests call the internal reader again with a previously admitted window.
fn canonical_staging(mut page: AgentChatPage) -> AgentChatPage {
    assert!(page.presentation.is_some());
    for event in &mut page.events {
        event.metadata["_wardian_canonical_row"] = json!(true);
    }
    page
}

#[test]
fn committed_turn_and_saved_call_tail_reconcile_through_the_projection_owner() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(candidate(turn_events(None)), "synthetic-running".into())
        .unwrap();
    settle(&owner);
    let running = read_turn(None);
    let projected = value(&running);
    assert_eq!(projected.turns.len(), 1);
    assert_eq!(projected.turns[0].step_count, 1);
    let step = projected
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Step)
        .unwrap();
    let item_key = step.item_key.clone();
    let order_key = step.order_key.clone();
    let saved_ref = step.step.as_ref().unwrap().detail_ref.clone().unwrap();
    let pending = value(&read_turn(Some(&saved_ref))).detail.unwrap();
    assert_eq!(pending.step_key, item_key);
    assert!(pending.pending);
    assert!(
        pending.text.is_empty(),
        "invocation input must never become output"
    );
    assert!(pending
        .input_text
        .as_ref()
        .is_some_and(|text| text.len() <= 1024));
    assert!(pending.input_truncated);
    let output = format!("{}END", "雪🙂".repeat(7000));
    owner
        .committed(
            candidate(turn_events(Some(&output))),
            "synthetic-completed".into(),
        )
        .unwrap();
    settle(&owner);
    let completed = read_turn(None);
    let projected = value(&completed);
    assert_eq!(projected.turns[0].status, ChatTurnStatus::Completed);
    assert_eq!(projected.turns[0].step_count, 1);
    assert!(projected.turns[0].answer_recorded);
    assert_eq!(projected.state_event_refs.len(), 1);
    assert!(!projected
        .items
        .iter()
        .any(|item| item.event_refs.contains(&"synthetic-terminal".into())));
    let step = projected
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Step)
        .unwrap();
    assert_eq!((&step.item_key, &step.order_key), (&item_key, &order_key));
    assert_eq!(
        step.step.as_ref().unwrap().description,
        "Run synthetic tests"
    );
    assert_eq!(
        step.step.as_ref().unwrap().status,
        ChatStepStatus::Completed
    );
    let tail = value(&read_turn(Some(&saved_ref))).detail.unwrap();
    assert_eq!(tail.step_key, item_key);
    assert!(tail.text.len() <= CHAT_STEP_OUTPUT_BYTES);
    assert!(output.ends_with(&tail.text));
    assert!(tail.text.ends_with("END"));
    assert!(tail.truncated_before);
    assert!(!tail.pending);
    assert!(!tail.complete);
    assert_eq!(tail.total_bytes, Some(output.len() as u64));
    assert!(serde_json::to_vec(&completed).unwrap().len() <= IPC_BYTES);
    drop(owner);
    let restarted = value(&read_turn(None));
    let restored_step = restarted
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Step)
        .unwrap();
    assert_eq!(
        (&restored_step.item_key, &restored_step.order_key),
        (&item_key, &order_key)
    );
    assert_eq!(restarted.turns[0].step_count, 1);
}

#[test]
fn missing_membership_keeps_the_existing_legacy_page() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let mut row = event("legacy-row", "An unmigrated canonical message.");
    row.sequence = Some(1);
    owner
        .committed(candidate(vec![row]), "synthetic-legacy".into())
        .unwrap();
    settle(&owner);
    let page = read_turn(None);
    assert!(page.presentation.is_none());
    assert_eq!(page.events.len(), 1);
}

#[test]
fn complete_browser_json_is_summarized_but_a_parseable_clipped_tail_stays_text() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let output = "{\"tabs\":[{\"id\":1},{\"id\":2}],\"private_field\":\"not a label\"}";
    let browser_rows = |output: &str| {
        let mut rows = turn_events(Some(output));
        rows[1].metadata["tool_name"] = json!("list_tabs");
        rows[2].language = Some("json".into());
        rows
    };
    owner
        .committed(candidate(browser_rows(output)), "synthetic-json".into())
        .unwrap();
    settle(&owner);
    let page = value(&read_turn(None));
    let reference = page
        .items
        .iter()
        .find_map(|item| item.step.as_ref().and_then(|step| step.detail_ref.clone()))
        .unwrap();
    let detail = value(&read_turn(Some(&reference))).detail.unwrap();
    assert_eq!(detail.format, ChatOutputFormat::Json);
    assert!(detail.complete);
    assert_eq!(detail.result_summary.as_deref(), Some("2 tabs"));
    assert_eq!(detail.text, output);
    let clipped = format!("{}{}", " ".repeat(CHAT_STEP_OUTPUT_BYTES + 100), output);
    owner
        .committed(
            candidate(browser_rows(&clipped)),
            "synthetic-json-grown".into(),
        )
        .unwrap();
    settle(&owner);
    let page = value(&read_turn(None));
    let reference = page
        .items
        .iter()
        .find_map(|item| item.step.as_ref().and_then(|step| step.detail_ref.clone()))
        .unwrap();
    let detail = value(&read_turn(Some(&reference))).detail.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&detail.text).is_ok());
    assert!(detail.truncated_before);
    assert!(!detail.complete);
    assert_eq!(detail.format, ChatOutputFormat::Text);
    assert!(detail.result_summary.is_none());
}

#[test]
fn recent_and_older_windows_keep_full_counts_for_a_large_turn() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let template = turn_events(Some("synthetic output"));
    let mut rows = vec![template[0].clone()];
    for i in 0..120 {
        let mut call = template[1].clone();
        call.id = format!("synthetic-call-{i}");
        call.turn_id = Some(format!("synthetic-invocation-{i}"));
        call.sequence = Some(2 * i + 2);
        call.metadata["tool_input"]
            .as_object_mut()
            .unwrap()
            .remove("context");
        let mut result = template[2].clone();
        result.id = format!("synthetic-result-{i}");
        result.turn_id = call.turn_id.clone();
        result.sequence = Some(2 * i + 3);
        rows.extend([call, result]);
    }
    let mut answer = template[3].clone();
    answer.sequence = Some(242);
    rows.push(answer);
    let mut terminal = template[4].clone();
    terminal.sequence = Some(243);
    rows.push(terminal);
    owner
        .committed(candidate(rows), "synthetic-large-turn".into())
        .unwrap();
    settle(&owner);
    let recent = read_turn(None);
    let recent_value = value(&recent);
    assert_eq!(recent_value.turns[0].step_count, 120);
    assert!(recent_value.turns[0].counts_complete);
    assert!(recent.events.len() <= PAGE_ROWS);
    let before = recent.next_before.as_deref().unwrap();
    let older = read(
        &context(),
        &snapshot(&context()),
        Some(before),
        Some(&recent.revision),
        None,
    )
    .unwrap();
    let older_value = value(&older);
    assert!(older.older_page);
    assert_eq!(older_value.turns.len(), 1);
    assert_eq!(
        older_value.turns[0].turn_key,
        recent_value.turns[0].turn_key
    );
    assert_eq!(older_value.turns[0].step_count, 120);
    assert!(older_value.turns[0].counts_complete);
    assert!(older.events.len() <= PAGE_ROWS);
    assert!(serde_json::to_vec(&older).unwrap().len() <= IPC_BYTES);
    assert!(older.records_decoded <= super::super::chat_read_store::READ_OBJECTS + 8);
}

#[test]
fn normalized_codex_request_membership_reaches_the_canonical_index() {
    let lines = [
        r#"{"type":"session_meta","payload":{"id":"synthetic-session"}}"#,
        r#"{"type":"turn_context","payload":{"turn_id":"synthetic-turn"}}"#,
        r#"{"type":"response_item","payload":{"type":"message","id":"synthetic-request","role":"user","content":[{"type":"input_text","text":"Run synthetic tests."}],"internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-turn","content_item_kinds":["user.text"]}}}"#,
        r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"synthetic-turn"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"synthetic-invocation","arguments":"{\"cmd\":\"echo synthetic\",\"description\":\"Run synthetic tests\"}"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"synthetic-invocation","output":"passed"}}"#,
        r#"{"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"The synthetic tests passed."}]}}"#,
        r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"synthetic-turn"}}"#,
    ];
    let events = crate::providers::chat_transcript::normalize_chat_lines("agent-1", "codex", lines);
    let temp = tempfile::tempdir().unwrap();
    let mut index = super::super::chat_turn_index::Index::default();
    let scope = super::super::chat_turn_index::Scope {
        agent_id: "agent-1",
        conversation_id: "synthetic-conversation",
        source_key: Some("codex:synthetic-session"),
        source_epoch: None,
        can_activate: true,
    };
    let mut store = Store::writer(temp.path());
    for event in &events {
        index
            .observe(
                &mut store,
                &scope,
                event,
                &presentation_order(event).unwrap(),
                None,
            )
            .unwrap();
    }
    let page = index
        .page(&mut Store::new(temp.path()), &events, true)
        .unwrap()
        .unwrap();
    assert_eq!(page.turns.len(), 1);
    assert_eq!(page.turns[0].step_count, 1);
    assert!(page.turns[0].answer_recorded);
    assert_eq!(page.turns[0].status, ChatTurnStatus::Completed);
    assert_eq!(
        page.items
            .iter()
            .filter(|item| item.kind == ChatPresentedItemKind::Step)
            .count(),
        1
    );
    assert!(page.items.iter().all(|item| item
        .step
        .as_ref()
        .is_none_or(|step| !step.description.contains("task_started"))));
}

#[test]
fn preview_and_foreign_scope_views_never_receive_canonical_capabilities() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(
            candidate(turn_events(Some("synthetic output"))),
            "synthetic-scope".into(),
        )
        .unwrap();
    settle(&owner);
    let canonical = canonical_staging(read_turn(None));
    assert!(canonical.presentation.is_some());
    let reference = head("agent-1").unwrap().unwrap();
    let mut reader = store("agent-1").unwrap();
    let published: Head = reader.read(&reference).unwrap();
    for change in [
        "recent",
        "provisional",
        "unresolved",
        "generation",
        "conversation",
        "epoch",
        "noncanonical",
    ] {
        let mut page = canonical.clone();
        page.presentation = None;
        match change {
            "recent" => page.recent_admission = Some("synthetic-preview-admission".into()),
            "provisional" => page.events[0].metadata["chat_provisional"] = json!(true),
            "unresolved" => {
                page.events[0].metadata["chat_identity_resolution"] = json!("unresolved")
            }
            "generation" => page.generation = Some("foreign-generation".into()),
            "conversation" => page.conversation_id = Some("foreign-conversation".into()),
            "noncanonical" => {
                page.events[0]
                    .metadata
                    .as_object_mut()
                    .unwrap()
                    .remove("_wardian_canonical_row");
            }
            _ => page.source_epoch = Some("foreign-epoch".into()),
        }
        let before = reader.objects;
        turn_read::attach(
            &mut reader,
            Some(&published),
            Some(&reference),
            true,
            &mut page,
        )
        .unwrap();
        assert!(page.presentation.is_none(), "{change}");
        assert_eq!(
            reader.objects, before,
            "rejected scope must not spend a metadata read"
        );
    }
}

#[test]
fn unsupported_or_malformed_packed_capability_falls_back_as_a_whole() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(
            candidate(turn_events(Some("synthetic output"))),
            "synthetic-fallback".into(),
        )
        .unwrap();
    settle(&owner);
    let canonical = canonical_staging(read_turn(None));
    assert!(canonical.presentation.is_some());
    let head_ref = head("agent-1").unwrap().unwrap();
    let mut reader = store("agent-1").unwrap();
    let published: Head = reader.read(&head_ref).unwrap();
    let original: Value = serde_json::from_slice(
        &fs::read(reader.dir.join(published.recent_page.as_ref().unwrap())).unwrap(),
    )
    .unwrap();
    for version in [1, 99] {
        let mut pack = original.clone();
        pack["presentation"] =
            json!({"version":version, "payload":{"not_a_supported_payload":true}});
        let bytes = serde_json::to_vec(&pack).unwrap();
        let reference = digest(&bytes);
        fs::write(reader.dir.join(&reference), bytes).unwrap();
        let mut head = published.clone();
        head.recent_page = Some(reference);
        let mut page = canonical.clone();
        page.presentation = None;
        let before = reader.objects;
        turn_read::attach(&mut reader, Some(&head), Some(&head_ref), true, &mut page).unwrap();
        assert!(page.presentation.is_none());
        assert!(
            reader.objects > before,
            "fallback must validate the supplied metadata"
        );
        assert_eq!(page.events, canonical.events);
    }
}

#[test]
fn editing_legacy_history_cannot_activate_partial_turn_totals() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(
            candidate(turn_events(Some("old output"))),
            "synthetic-before-upgrade".into(),
        )
        .unwrap();
    settle(&owner);
    // Model a compatible legacy checkpoint without derived turn metadata. A
    // complete rebuild after process restart may safely index every old row;
    // incremental renewal must not activate totals from changed rows alone.
    let (head_path, objects) = locations("agent-1").unwrap();
    let mut writer = Store::writer(&objects);
    let mut legacy: Head = writer.read(&head("agent-1").unwrap().unwrap()).unwrap();
    legacy.turns = Default::default();
    for (_, reference) in writer.page(&legacy.root, None, PAGE_ROWS).unwrap() {
        let mut row: Row = writer.read(&reference).unwrap();
        row.event
            .metadata
            .as_object_mut()
            .unwrap()
            .remove("chat_turn_key");
        row.event
            .metadata
            .as_object_mut()
            .unwrap()
            .remove("chat_turn_item_key");
        insert_row(&mut writer, &mut legacy, &row).unwrap();
    }
    legacy.parent = None;
    legacy.changes.clear();
    legacy.recent_page = None;
    legacy.recent_page = packed_recent::prepare(&mut writer, &legacy).unwrap();
    let reference = writer.put(&legacy).unwrap();
    write_json_atomic(&head_path, &reference).unwrap();
    {
        let mut work = owner.work.lock().unwrap();
        let current = work.get_mut("agent-1").unwrap();
        current.head = legacy;
        current.published = Some(reference);
    }
    let old_rows = turn_events(Some("revised old output"));
    owner
        .committed(
            candidate(old_rows.clone()),
            "synthetic-after-upgrade".into(),
        )
        .unwrap();
    settle(&owner);
    assert!(read_turn(None).presentation.is_none());
    let mut rows = old_rows;
    for mut event in turn_events(Some("new output")) {
        event.id = format!("new-{}", event.id);
        event.sequence = event.sequence.map(|value| value + 5);
        event.metadata["request_root_id"] = json!("synthetic-new-request");
        rows.push(event);
    }
    owner
        .committed(candidate(rows), "synthetic-new-request".into())
        .unwrap();
    settle(&owner);
    let current = value(&read_turn(None));
    assert_eq!(current.turns.len(), 1);
    assert_eq!(current.turns[0].step_count, 1);
    assert!(current.turns[0].counts_complete);
    assert!(!current.legacy_turns.is_empty());
}

#[test]
fn source_metadata_cannot_issue_canonical_item_membership() {
    let (_guard, _temp) = isolated_home();
    let mut events = turn_events(Some("synthetic output"));
    let forged = "a".repeat(64);
    for event in &mut events {
        event.metadata["chat_turn_key"] = json!(forged);
        event.metadata["chat_turn_item_key"] = json!(forged);
        event.metadata["_wardian_canonical_row"] = json!(true);
        let clipped = header(event);
        assert!(clipped.metadata.get("chat_turn_key").is_none());
        assert!(clipped.metadata.get("chat_turn_item_key").is_none());
        assert!(clipped.metadata.get("_wardian_canonical_row").is_none());
    }
    let owner = ProjectionOwner::default();
    owner
        .committed(candidate(events), "synthetic-membership".into())
        .unwrap();
    settle(&owner);
    let page = read_turn(None);
    let projected = value(&page);
    for event in &page.events {
        assert!(event.metadata.get("_wardian_canonical_row").is_none());
        assert_ne!(
            event.metadata["chat_turn_key"].as_str(),
            Some(forged.as_str())
        );
        let expected = projected
            .items
            .iter()
            .find(|item| item.event_refs.contains(&event.id));
        if let Some(item) = expected {
            assert_eq!(
                event.metadata["chat_turn_item_key"].as_str(),
                Some(item.item_key.as_str())
            );
        }
    }
}

#[test]
fn native_status_only_closure_enriches_the_owned_request_after_restart() {
    let (_guard, temp) = isolated_home();
    let header = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"synthetic-session\"}}\n";
    let path = temp.path().join("synthetic-codex-closure.jsonl");
    let (snapshot, _) = super::tests::owned_native_source_for_provider_session(
        &path,
        header,
        "agent-1",
        "synthetic-session",
    );
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let capture = |previous| {
        let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            context.provider_source_key.as_deref().unwrap(),
            previous,
            true,
        )
        .unwrap();
        crate::commands::chat::decorate_forward_provider_log_events(
            &mut batch.events,
            "codex",
            &path,
        );
        batch
    };
    let prime = capture(None);
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &prime.events,
            None,
            &prime.next,
            prime.source_proof(),
        )
        .unwrap();
    assert!(archive.active_conversation_id("agent-1").unwrap().is_none());
    let fixture = include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl");
    let commentary = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","phase":"commentary","turn_id":"provider-turn-a","content":[{"type":"output_text","text":"Still running."}]}}"#;
    let prefix = format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\",\"turn_id\":\"provider-turn-a\"}}}}\n{fixture}\n{commentary}\n");
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(prefix.as_bytes())
        .unwrap();
    let live = capture(Some(prime.next.clone()));
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &live.events,
            Some(&prime.next),
            &live.next,
            live.source_proof(),
        )
        .unwrap();
    super::tests::finish_projection(&archive);
    // The live recent preview can still contain an uncommitted mirror. Demand
    // canonical history explicitly; preview identity is a separate capability.
    let canonical_page = || {
        let head_ref = head("agent-1").unwrap().unwrap();
        let mut reader = store("agent-1").unwrap();
        let published: Head = reader.read(&head_ref).unwrap();
        let (_, cursor) = page_limited(&mut reader, &head_ref, &published, None, 1).unwrap();
        read(
            &context,
            &snapshot,
            Some(cursor.as_deref().unwrap()),
            None,
            None,
        )
        .unwrap()
    };
    let before = value(&canonical_page());
    assert!(!before.turns[0].counts_complete);
    let prompt_before = before
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Prompt)
        .unwrap();
    let conversation = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let directory = super::super::conversation_dir("agent-1", &conversation).unwrap();
    let records_before = fs::read(directory.join("conversation.jsonl")).unwrap();
    // An ordinary ingress path cannot publish capture-owned lifecycle metadata.
    let mut forged = live
        .events
        .iter()
        .find(|event| event.role == Some(AgentChatRole::User))
        .unwrap()
        .clone();
    forged.metadata[super::super::chat_turn_closure::METADATA_KEY] =
        json!({"version":1,"complete":true});
    archive
        .append_chat_events_with_context(context.clone(), &[forged])
        .unwrap();
    let stored: Vec<AgentChatEvent> =
        wardian_core::conversations::read_jsonl_records(&directory.join("events.jsonl")).unwrap();
    assert!(stored
        .iter()
        .all(|event| event.metadata[super::super::chat_turn_closure::METADATA_KEY].is_null()));
    let closing = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"provider-turn-a\"}}\n";
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(closing.as_bytes())
        .unwrap();
    let closed = capture(Some(live.next.clone()));
    assert_eq!(closed.events.len(), 1);
    assert_eq!(closed.events[0].kind, AgentChatEventKind::Status);
    // Restart drops the mutable writer handle. The status-only owner lookup
    // must find the existing source on disk without allocating another turn.
    let restarted = ConversationArchiveState::default();
    restarted
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &closed.events,
            Some(&live.next),
            &closed.next,
            closed.source_proof(),
        )
        .unwrap();
    super::tests::finish_projection(&restarted);
    assert_eq!(
        fs::read(directory.join("conversation.jsonl")).unwrap(),
        records_before
    );
    let page = canonical_page();
    let after = value(&page);
    assert!(after.turns[0].counts_complete);
    assert_eq!(after.turns[0].status, ChatTurnStatus::Completed);
    assert_eq!(after.turns[0].turn_key, before.turns[0].turn_key);
    let prompt_after = after
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Prompt)
        .unwrap();
    assert_eq!(
        (&prompt_after.item_key, &prompt_after.order_key),
        (&prompt_before.item_key, &prompt_before.order_key)
    );
    assert!(page
        .events
        .iter()
        .all(|event| event.metadata[super::super::chat_turn_closure::METADATA_KEY].is_null()));
}

#[test]
fn source_proven_closing_status_cannot_create_an_orphan_conversation() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("synthetic-codex-orphan.jsonl");
    let (snapshot, _) = super::tests::owned_native_source_for_provider_session(
        &path,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"synthetic-session\"}}\n",
        "agent-1",
        "synthetic-session",
    );
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let prime = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &prime.events,
            None,
            &prime.next,
            prime.source_proof(),
        )
        .unwrap();
    let fixture = include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl");
    let contents = format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\",\"turn_id\":\"provider-turn-a\"}}}}\n{fixture}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\",\"turn_id\":\"provider-turn-a\"}}}}\n");
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(contents.as_bytes())
        .unwrap();
    let batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(prime.next.clone()),
        true,
    )
    .unwrap();
    let statuses: Vec<_> = batch
        .events
        .iter()
        .filter(|event| event.kind == AgentChatEventKind::Status)
        .cloned()
        .collect();
    assert_eq!(statuses.len(), 1);
    assert!(batch.source_proof().unwrap().proves(&statuses[0]));
    archive
        .append_verified_provider_log_batch_with_context(
            context,
            &statuses,
            Some(&prime.next),
            &batch.next,
            batch.source_proof(),
        )
        .unwrap();
    assert!(archive.active_conversation_id("agent-1").unwrap().is_none());
    assert!(archive.list(Some("agent-1"), false).unwrap().is_empty());
    assert!(head("agent-1").unwrap().is_none());
}

#[test]
fn normalized_codex_mirrors_keep_canonical_presentation_on_public_recent_reads() {
    let (_guard, temp) = isolated_home();
    let fixture = include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl");
    let suffix = [
        r#"{"type":"response_item","payload":{"type":"function_call","turn_id":"provider-turn-a","name":"exec_command","call_id":"synthetic-one","arguments":"{\"cmd\":\"echo synthetic\"}"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call","turn_id":"provider-turn-a","name":"exec_command","call_id":"synthetic-two","arguments":"{\"cmd\":\"echo parallel\"}"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call_output","turn_id":"provider-turn-a","call_id":"synthetic-one","output":"passed"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call_output","turn_id":"provider-turn-a","call_id":"synthetic-two","output":"passed"}}"#,
        r#"{"type":"event_msg","payload":{"type":"agent_message","turn_id":"provider-turn-a","message":"Synthetic answer."}}"#,
        r#"{"type":"response_item","payload":{"type":"message","id":"synthetic-answer","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Synthetic answer."}],"internal_chat_message_metadata_passthrough":{"turn_id":"provider-turn-a","content_item_kinds":["unknown"]}}}"#,
        r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"provider-turn-a"}}"#,
        r#"{"type":"response_item","payload":{"type":"message","id":"unrooted-neighbour","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"Unrooted neighbouring work."}],"internal_chat_message_metadata_passthrough":{"turn_id":"foreign-turn","content_item_kinds":["unknown"]}}}"#,
    ].join("\n");
    let header = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"synthetic-session\"}}\n";
    let contents = format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\",\"turn_id\":\"provider-turn-a\"}}}}\n{fixture}\n{suffix}\n");
    let path = temp.path().join("synthetic-codex-mirrors.jsonl");
    let (snapshot, _) = super::tests::owned_native_source_for_provider_session(
        &path,
        header,
        "agent-1",
        "synthetic-session",
    );
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let primed = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &primed.events,
            None,
            &primed.next,
            primed.source_proof(),
        )
        .unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(contents.as_bytes())
        .unwrap();
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(primed.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
    assert!(
        batch.source_proof().is_some(),
        "live capture must carry an opaque source receipt"
    );
    let proof = batch.source_proof().unwrap();
    let terminal = batch
        .events
        .iter()
        .find(|event| event.kind == AgentChatEventKind::Status)
        .unwrap();
    assert!(proof.proves(terminal));
    for (key, value) in [
        ("request_root_id", json!("unproven-root")),
        ("chat_turn_adapter", json!("unproven-adapter")),
        ("chat_turn_membership_complete", json!(false)),
        ("chat_turn_membership_partial", json!(true)),
    ] {
        let mut altered = terminal.clone();
        altered.metadata[key] = value;
        assert!(!proof.proves(&altered), "receipt must bind {key}");
    }
    let mut altered = terminal.clone();
    altered.status = Some(AgentChatStatus::Failed);
    assert!(!proof.proves(&altered));
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &batch.events,
            Some(&primed.next),
            &batch.next,
            batch.source_proof(),
        )
        .unwrap();
    super::tests::finish_projection(&archive);
    // The preview identity bridge is deferred. Exercise a canonical history
    // demand using the cursor issued by the existing bounded canonical reader.
    let head_ref = head("agent-1").unwrap().unwrap();
    let mut reader = store("agent-1").unwrap();
    let published: Head = reader.read(&head_ref).unwrap();
    let (_, cursor) = page_limited(&mut reader, &head_ref, &published, None, 1).unwrap();
    let page = read(
        &context,
        &snapshot,
        Some(cursor.as_deref().unwrap()),
        None,
        None,
    )
    .unwrap();
    let presentation = value(&page);
    assert_eq!(presentation.turns.len(), 1);
    assert_eq!(presentation.turns[0].step_count, 2);
    assert!(page.older_page);
    for call in ["synthetic-one", "synthetic-two"] {
        let invocation = batch
            .events
            .iter()
            .find(|event| {
                event.kind == AgentChatEventKind::ToolCall && event.metadata["call_id"] == call
            })
            .unwrap();
        let step = presentation
            .items
            .iter()
            .find(|item| {
                item.step.as_ref().is_some_and(|step| {
                    step.invocation_ref.as_deref() == Some(invocation.id.as_str())
                })
            })
            .unwrap();
        assert!(step
            .order_key
            .starts_with(&presentation_order(invocation).unwrap()));
    }
    assert!(presentation.turns[0].counts_complete);
    assert!(presentation.turns[0].answer_recorded);
    assert_eq!(
        presentation
            .items
            .iter()
            .filter(|item| item.kind == ChatPresentedItemKind::Prompt)
            .count(),
        1
    );
    assert_eq!(
        presentation
            .items
            .iter()
            .filter(|item| item.kind == ChatPresentedItemKind::Answer)
            .count(),
        1
    );
    assert!(
        page.aliases.len() >= 2,
        "fixture must exercise certified physical mirrors"
    );
    super::tests::finish_source_index(&snapshot);
    let preview = read(&context, &snapshot, None, None, None).unwrap();
    assert!(
        preview.presentation.is_none(),
        "the native preview scope waits for its identity bridge"
    );
}

#[test]
fn claude_parent_chain_composition_publishes_partial_live_and_complete_closed_counts() {
    let (_guard, _temp) = isolated_home();
    let prefix = [
        r#"{"type":"user","uuid":"request","sessionId":"synthetic-session","parentUuid":null,"message":{"role":"user","content":"Run synthetic tests."}}"#,
        r#"{"type":"assistant","uuid":"call","sessionId":"synthetic-session","parentUuid":"request","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"echo synthetic"}}],"stop_reason":"tool_use"}}"#,
        r#"{"type":"user","uuid":"result","sessionId":"synthetic-session","parentUuid":"call","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"passed"}]}}"#,
    ];
    let final_row = r#"{"type":"assistant","uuid":"answer","sessionId":"synthetic-session","parentUuid":"result","message":{"role":"assistant","content":[{"type":"text","text":"The synthetic tests passed."}],"stop_reason":"end_turn"}}"#;
    let broken = r#"{"type":"assistant","uuid":"broken","sessionId":"synthetic-session","parentUuid":"missing","message":{"role":"assistant","content":[{"type":"text","text":"The synthetic tests passed."}]}}"#;
    let mut context = ConversationArchiveContext::for_agent_id("agent-1", "claude");
    context.provider_source_key = Some("claude:synthetic".into());
    let owner = ProjectionOwner::default();
    for (stamp, suffix, complete) in [
        ("synthetic-live", vec![], false),
        ("synthetic-complete", vec![final_row], true),
        ("synthetic-broken", vec![broken, final_row], false),
    ] {
        let mut events = crate::providers::chat_transcript::normalize_chat_lines(
            "agent-1",
            "claude",
            prefix.into_iter().chain(suffix),
        );
        for event in &mut events {
            if let Some(uuid) = event.metadata
                [crate::providers::chat_transcript::PROVIDER_EVENT_ID_METADATA_KEY]
                .as_str()
            {
                event.id = format!("fixture:claude:{uuid}");
            }
        }
        owner
            .committed(
                Candidate {
                    context: context.clone(),
                    ..base_candidate(events)
                },
                stamp.into(),
            )
            .unwrap();
        settle(&owner);
        let page = read(&context, &snapshot(&context), None, None, None).unwrap();
        let presentation = value(&page);
        assert_eq!(presentation.turns.len(), 1);
        assert_eq!(presentation.turns[0].step_count, 1);
        assert_eq!(presentation.turns[0].counts_complete, complete);
        assert_eq!(presentation.turns[0].membership_complete, complete);
        if stamp == "synthetic-broken" {
            assert!(presentation
                .legacy_turns
                .iter()
                .any(|group| group.kind == ChatLegacyTurnKind::EarlierActivity));
        }
    }
}

#[test]
fn claude_combined_native_results_cannot_publish_complete_work_evidence() {
    let (_guard, _temp) = isolated_home();
    let fixture =
        include_str!("../../providers/fixtures/claude-parent-chain-combined-results.jsonl");
    let events = crate::providers::chat_transcript::normalize_chat_lines(
        "agent-1",
        "claude",
        fixture.lines(),
    );
    let mut context = ConversationArchiveContext::for_agent_id("agent-1", "claude");
    context.provider_source_key = Some("claude:synthetic".into());
    let owner = ProjectionOwner::default();
    owner
        .committed(
            Candidate {
                context: context.clone(),
                ..base_candidate(events)
            },
            "synthetic-combined-results".into(),
        )
        .unwrap();
    settle(&owner);
    let page = read(&context, &snapshot(&context), None, None, None).unwrap();
    let presentation = value(&page);
    assert_eq!(presentation.turns[0].step_count, 2);
    assert!(!presentation.turns[0].counts_complete);
    assert!(!presentation.turns[0].membership_complete);
    assert_eq!(
        presentation
            .items
            .iter()
            .filter_map(|item| item.step.as_ref())
            .filter(|step| step.result_ref.is_some())
            .count(),
        1
    );
    assert_eq!(
        page.events
            .iter()
            .filter(|event| event.kind == AgentChatEventKind::ToolResult)
            .count(),
        1
    );
}
