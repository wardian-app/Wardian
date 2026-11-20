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

#[test]
fn recent_cold_first_miss_skips_source_scope_io() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));

    for pending in [false, true] {
        if pending {
            owner.begin_recent(&source, &runtime).unwrap();
        }
        owner
            .recent_scope_reads
            .store(0, std::sync::atomic::Ordering::SeqCst);
        let page = owner.recent_reader_with_hints(
            source.clone(),
            runtime.clone(),
            None,
            8,
            Some(Vec::new()),
        )()
        .unwrap();
        assert!(
            page.is_none(),
            "an unpublished owner cannot serve a recent page"
        );
        assert_eq!(
            owner
                .recent_scope_reads
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "cold misses must reach the ordinary reader without source-certificate IO"
        );
    }
}

#[test]
fn recent_cold_first_miss_preserves_existing_reader_and_privacy_paths() {
    let (_guard, temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));

    for (revision, hints) in [
        (None, None),
        (None, Some(vec!["literal-preview-backing".to_owned()])),
        (
            Some("ordinary-canonical-revision".to_owned()),
            Some(Vec::new()),
        ),
    ] {
        owner
            .recent_scope_reads
            .store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(owner.recent_reader_with_hints(
            source.clone(),
            runtime.clone(),
            revision,
            8,
            hints,
        )()
        .unwrap()
        .is_none());
        assert_eq!(
            owner
                .recent_scope_reads
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    for agent in [Some("agent-1"), None] {
        let fence = owner.recent_boundary(agent).unwrap();
        owner
            .recent_scope_reads
            .store(0, std::sync::atomic::Ordering::SeqCst);
        let page = owner.recent_reader_with_hints(
            source.clone(),
            runtime.clone(),
            None,
            8,
            Some(Vec::new()),
        )()
        .unwrap()
        .unwrap();
        assert!(page.events.is_empty() && page.reset);
        assert_eq!(page.progress, "source_changed");
        assert_eq!(
            owner
                .recent_scope_reads
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        drop(fence);
    }

    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    owner
        .publish_recent(
            &source,
            &runtime,
            &receipt,
            vec![event("published-preview", "Synthetic preview output.")],
        )
        .unwrap();
    owner
        .recent_scope_reads
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let page =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    assert_eq!(page.recent_admission.as_deref(), Some(receipt.0.as_str()));
    assert!(!page.events.is_empty());
    assert_eq!(
        owner
            .recent_scope_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    owner.retire_recent(&source.session_id).unwrap();
    owner
        .recent_scope_reads
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let retired = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(page.revision),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert!(retired.events.is_empty() && retired.reset);
    assert!(retired.recent_admission.is_none());
    assert_eq!(
        owner
            .recent_scope_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    let native_path = temp.path().join("native-watch.log");
    fs::write(&native_path, b"Synthetic native observation.").unwrap();
    let native = crate::commands::chat_recent_database::Source::watch(&native_path).unwrap();
    owner
        .begin_recent_with_source(&source, &runtime, Some(native))
        .unwrap();
    owner
        .recent_scope_reads
        .store(0, std::sync::atomic::Ordering::SeqCst);
    assert!(owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        None,
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .is_none());
    assert_eq!(
        owner
            .recent_scope_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    fs::write(&native_path, b"x").unwrap();
    owner
        .recent_scope_reads
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let changed =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    assert!(changed.events.is_empty() && changed.reset);
    assert_eq!(changed.progress, "source_changed");
    assert_eq!(
        owner
            .recent_scope_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    owner
        .recent_scope_reads
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let invalid =
        owner.recent_reader_with_hints(source, runtime, None, 0, Some(Vec::new()))().unwrap_err();
    assert_eq!(invalid.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(
        owner
            .recent_scope_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[test]
fn recent_cold_first_miss_keeps_another_runtime_owners_admission() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let original_runtime = Arc::new(Mutex::new("Idle".into()));
    let receipt = owner.begin_recent(&source, &original_runtime).unwrap();
    owner
        .publish_recent(
            &source,
            &original_runtime,
            &receipt,
            vec![event("original-preview", "Synthetic original output.")],
        )
        .unwrap();

    owner
        .recent_scope_reads
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let replacement_runtime = Arc::new(Mutex::new("Idle".into()));
    assert!(owner.recent_reader_with_hints(
        source.clone(),
        replacement_runtime,
        None,
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .is_none());
    assert_eq!(
        owner
            .recent_scope_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    let original =
        owner.recent_reader_with_hints(source, original_runtime, None, 8, Some(Vec::new()))()
            .unwrap()
            .unwrap();
    assert_eq!(
        original.recent_admission.as_deref(),
        Some(receipt.0.as_str())
    );
    assert_eq!(
        original.events[0].text.as_deref(),
        Some("Synthetic original output.")
    );
    assert_eq!(
        owner
            .recent_scope_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[test]
fn recent_identity_handoff_is_one_owned_revision_and_privacy_retires_its_receipt() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    let mut events = turn_events(Some("Synthetic tests passed"));
    let mut tags = HashMap::new();
    let headers = events
        .iter_mut()
        .enumerate()
        .map(|(index, event)| {
            let key = format!("{}:watch:{index}", receipt.0);
            let version = format!("version-{index}");
            event.metadata["chat_recent_key"] = json!(key);
            event.metadata["chat_recent_version"] = json!(version);
            tags.insert(event.id.clone(), (key, version));
            let mut header = header(event);
            header.id = format!("preview:{}:{index}", receipt.0);
            header
        })
        .collect();
    owner
        .publish_recent(&source, &runtime, &receipt, headers)
        .unwrap();
    owner.note_recent_coverage(&receipt, &tags).unwrap();
    let cold =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    assert!(cold.presentation.is_none());
    assert!(cold.conversation_id.is_none());
    let mut candidate = candidate(events);
    candidate.context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    owner
        .committed(candidate, "synthetic-covered".into())
        .unwrap();
    settle(&owner);
    let switched = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(cold.revision),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    let presented = value(&switched);
    assert_eq!(
        presented.preview_bridge.as_ref().unwrap().admission_id,
        receipt.0
    );
    assert_eq!(switched.aliases.len(), switched.events.len());
    assert!(switched.recent_admission.is_none());
    assert!(presented.turns.iter().all(|turn| turn.membership_complete));
    let repeated = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(switched.revision),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert_eq!(presented.turns, value(&repeated).turns);
    assert_eq!(presented.items, value(&repeated).items);
    let boundary = owner.recent_boundary(Some("agent-1")).unwrap();
    assert!(!owner
        .publish_recent(&source, &runtime, &receipt, cold.events)
        .unwrap());
    let retired =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    assert!(retired.events.is_empty());
    assert!(retired.presentation.is_none());
    drop(boundary);
    let next = owner.begin_recent(&source, &runtime).unwrap();
    assert_ne!(next.0, receipt.0);
    assert!(!owner
        .publish_recent(&source, &runtime, &receipt, Vec::new())
        .unwrap());
}

fn read_turn(detail: Option<&str>) -> AgentChatPage {
    read(&context(), &snapshot(&context()), None, None, detail).unwrap()
}

#[test]
fn one_readers_binding_cannot_waive_another_readers_legacy_aliases() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    let mut events = turn_events(Some("Synthetic success"));
    let key = format!("{}:watch:1", receipt.0);
    events[3].metadata["chat_recent_key"] = json!(key);
    events[3].metadata["chat_recent_version"] = json!("one");
    let canonical_id = events[3].id.clone();
    let mut preview = header(&events[3]);
    preview.id = "legacy-reader-answer".into();
    owner
        .publish_recent(&source, &runtime, &receipt, vec![preview])
        .unwrap();
    let legacy =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    let mut candidate = candidate(events);
    candidate.context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    owner
        .committed(candidate, "shared-first-view".into())
        .unwrap();
    settle(&owner);
    let first_view =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    assert!(value(&first_view).preview_bridge.is_some());
    let still_legacy = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(legacy.revision.clone()),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert!(still_legacy.presentation.is_none());
    assert!(still_legacy.unchanged && still_legacy.aliases.is_empty());
    let still_bound = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(first_view.revision),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert!(value(&still_bound).preview_bridge.is_some());
    owner
        .note_recent_coverage(
            &receipt,
            &HashMap::from([(canonical_id.clone(), (key, "one".into()))]),
        )
        .unwrap();
    let switched = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(legacy.revision.clone()),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert!(value(&switched).preview_bridge.is_some());
    assert!(switched
        .aliases
        .iter()
        .any(|alias| alias.observation_id == "legacy-reader-answer"
            && alias.canonical_id == canonical_id));
    let (head_path, objects) = locations("agent-1").unwrap();
    let mut writer = Store::writer(&objects);
    let mut replaced: Head = writer.read(&head("agent-1").unwrap().unwrap()).unwrap();
    replaced.generation = "replaced-canonical-scope".into();
    write_json_atomic(&head_path, &writer.put(&replaced).unwrap()).unwrap();
    let retired = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(legacy.revision),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert!(retired.reset && retired.presentation.is_none() && retired.events.is_empty());
    assert!(!owner
        .publish_recent(&source, &runtime, &receipt, Vec::new())
        .unwrap());
}

#[test]
fn coverage_from_another_head_cannot_issue_a_handoff_even_with_identical_scope() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    let mut events = turn_events(Some("Synthetic success"));
    let key = format!("{}:watch:1", receipt.0);
    events[3].metadata["chat_recent_key"] = json!(key);
    events[3].metadata["chat_recent_version"] = json!("one");
    let canonical_id = events[3].id.clone();
    let mut preview = header(&events[3]);
    preview.id = "legacy-answer".into();
    let mut candidate = candidate(events);
    candidate.context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    owner.committed(candidate, "first-head".into()).unwrap();
    settle(&owner);
    let coverage = [RecentCoverage {
        event_id: canonical_id,
        key,
        version: "one".into(),
        native_key: None,
    }];
    let proof = recent_canonical_proof(&source, &coverage)
        .unwrap()
        .0
        .unwrap();
    let (head_path, objects) = locations("agent-1").unwrap();
    let mut writer = Store::writer(&objects);
    let mut next: Head = writer.read(&head("agent-1").unwrap().unwrap()).unwrap();
    next.capture_stamp = "different-publication-same-scope".into();
    next.recent_page = None;
    let next_ref = writer.put(&next).unwrap();
    write_json_atomic(&head_path, &next_ref).unwrap();
    assert!(preview_bridge::read(
        &source,
        &receipt.0,
        std::slice::from_ref(&preview),
        None,
        Some(&proof),
        8,
        preview_bridge::Selection {
            mode: preview_bridge::Mode::Legacy,
            hints: None
        }
    )
    .unwrap()
    .is_none());
    let current = recent_canonical_proof(&source, &coverage)
        .unwrap()
        .0
        .unwrap();
    assert!(preview_bridge::read(
        &source,
        &receipt.0,
        &[preview],
        None,
        Some(&current),
        8,
        preview_bridge::Selection {
            mode: preview_bridge::Mode::Legacy,
            hints: None
        }
    )
    .unwrap()
    .is_some());
}

#[test]
fn canonical_older_pages_retain_the_owned_bridge_with_unmigrated_history() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    let mut recent = turn_events(Some("Synthetic success"));
    for event in &mut recent {
        event.sequence = event.sequence.map(|value| value + 100);
    }
    recent[3].metadata["chat_recent_key"] = json!(format!("{}:watch:1", receipt.0));
    recent[3].metadata["chat_recent_version"] = json!("one");
    let mut preview = header(&recent[3]);
    preview.id = "preview-answer".into();
    owner
        .publish_recent(&source, &runtime, &receipt, vec![preview])
        .unwrap();
    owner
        .note_recent_coverage(
            &receipt,
            &HashMap::from([(
                recent[3].id.clone(),
                (format!("{}:watch:1", receipt.0), "one".into()),
            )]),
        )
        .unwrap();
    let mut events: Vec<_> = (0..90)
        .map(|index| {
            let mut row = event(&format!("unmigrated-{index}"), "Earlier synthetic activity");
            row.sequence = Some(index);
            row
        })
        .collect();
    events.extend(recent);
    let mut candidate = candidate(events);
    candidate.context = ctx.clone();
    owner
        .committed(candidate, "synthetic-older".into())
        .unwrap();
    settle(&owner);
    let current =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 5, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    assert!(value(&current).preview_bridge.is_some());
    let mut older = read_with_revalidation_and_limit(
        &ctx,
        &source,
        current.next_before.as_deref(),
        Some(&preview_bridge::split_revision(&current.revision).unwrap().0),
        None,
        None,
        Some(5),
    )
    .unwrap();
    assert!(older.older_page && older.presentation.is_none());
    let events = older.events.clone();
    let cursor = older.next_before.clone();
    older = owner
        .canonical_bridge_reader(
            source.clone(),
            runtime.clone(),
            RetiredChatRead {
                cursor: current.next_before.clone(),
                revision: Some(current.revision.clone()),
                detail_ref: None,
                revalidate_ids: None,
                limit: Some(5),
            },
        )
        .unwrap()()
    .unwrap();
    assert!(!value(&older).legacy_turns.is_empty());
    assert_eq!(
        value(&older).preview_bridge.unwrap().admission_id,
        receipt.0
    );
    assert_eq!(older.events, events);
    assert_eq!(older.next_before, cursor);
    assert!(older.recent_admission.is_none());
}

#[test]
fn normalized_running_source_closes_in_place_and_replacement_retracts_the_bridge() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("preview-source.jsonl");
    let mut lines = vec![
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn-one"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"turn-one"}}),
        json!({"type":"response_item","payload":{"type":"message","id":"prompt-one","role":"user","content":[{"type":"input_text","text":"Run synthetic tests."}],"internal_chat_message_metadata_passthrough":{"turn_id":"turn-one","content_item_kinds":["user.text"]}}}),
    ];
    for index in 0..10 {
        lines.push(json!({"type":"event_msg","payload":{"type":"agent_message","turn_id":"turn-one","message":format!("Synthetic progress {index}")}}));
    }
    lines.push(json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Synthetic answer waiting for closure."}],"internal_chat_message_metadata_passthrough":{"turn_id":"turn-one"}}}));
    let contents = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let (mut source, _) =
        super::tests::owned_native_source_for_provider_session(&path, &contents, "agent-1", "one");
    source.current_status = "Processing".into();
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    let native_legacy =
        read_with_revalidation_and_limit(&context, &source, None, None, None, None, Some(8))
            .unwrap();
    assert!(native_legacy.conversation_id.is_none() && native_legacy.presentation.is_none());
    let native_backing: Vec<_> = native_legacy
        .events
        .iter()
        .map(|event| event.id.clone())
        .collect();
    assert!(!native_backing.is_empty() && native_backing.len() <= 8);
    let archive = ConversationArchiveState::default();
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &batch.events,
            None,
            &batch.next,
            batch.source_proof(),
        )
        .unwrap();
    super::tests::finish_projection(&archive);
    let owner = &archive.chat_projection;
    let runtime = Arc::new(Mutex::new("Processing".into()));
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    let mut preview = event(
        "watch-only-preview",
        "Synthetic answer waiting for closure.",
    );
    preview.metadata =
        json!({"chat_recent_key":format!("{}:watch:1",receipt.0),"chat_recent_version":"one"});
    owner
        .publish_recent(&source, &runtime, &receipt, vec![preview])
        .unwrap();
    let running = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(native_legacy.revision),
        8,
        Some(native_backing.clone()),
    )()
    .unwrap()
    .unwrap();
    assert!(native_backing.iter().all(|id| running
        .aliases
        .iter()
        .any(|alias| &alias.observation_id == id)));
    let presented = value(&running);
    assert_eq!(presented.turns.len(), 1);
    assert!(presented.turns[0].prefix_attributed);
    assert!(!presented.turns[0].membership_complete);
    assert_eq!(presented.turns[0].status, ChatTurnStatus::Running);
    assert!(presented.turns[0].answer_item_keys.is_empty());
    assert!(
        running.next_before.is_none()
            && running.detail.is_none()
            && running.revalidated_ids.is_none()
    );
    assert_eq!(
        running.recent_admission.as_deref(),
        Some(receipt.0.as_str())
    );
    let answer_before = presented
        .items
        .iter()
        .find(|item| {
            item.preview
                .as_ref()
                .is_some_and(|preview| preview.text == "Synthetic answer waiting for closure.")
        })
        .unwrap();
    assert_eq!(answer_before.kind, ChatPresentedItemKind::Narration);
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
        "{}",
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-one"}})
    )
    .unwrap();
    let mut closed = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(batch.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut closed.events, "codex", &path);
    assert!(
        closed
            .events
            .iter()
            .any(|event| event.metadata["raw_type"] == "task_complete"
                && event.metadata["chat_turn_membership_complete"] == true
                && event.metadata["chat_turn_after_closed"] != true),
        "exact closure must survive source normalization"
    );
    archive
        .append_verified_provider_log_batch_with_context(
            context,
            &closed.events,
            Some(&batch.next),
            &closed.next,
            closed.source_proof(),
        )
        .unwrap();
    super::tests::finish_projection(&archive);
    source.current_status = "Idle".into();
    let completed = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(running.revision),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert_eq!(running.conversation_id, completed.conversation_id);
    assert_eq!(
        running.generation, completed.generation,
        "capture must preserve the current admission generation"
    );
    let final_view = value(&completed);
    let returned_ids: BTreeSet<_> = completed
        .events
        .iter()
        .map(|event| event.id.as_str())
        .collect();
    assert!(
        completed
            .aliases
            .iter()
            .all(|alias| returned_ids.contains(alias.canonical_id.as_str())),
        "the atomic bridge must normalize aliases after the ordinary reader bounds its window"
    );
    assert_eq!(running.conversation_id, completed.conversation_id);
    assert_eq!(running.generation, completed.generation);
    assert_eq!(running.source_epoch, completed.source_epoch);
    assert_ne!(
        presented.preview_bridge.as_ref().unwrap().canonical_head,
        final_view.preview_bridge.as_ref().unwrap().canonical_head
    );
    assert_eq!(final_view.turns[0].turn_key, presented.turns[0].turn_key);
    assert_eq!(final_view.turns[0].status, ChatTurnStatus::Completed);
    let answer = final_view
        .items
        .iter()
        .find(|item| item.item_key == answer_before.item_key)
        .unwrap();
    assert_eq!(answer.kind, ChatPresentedItemKind::Answer);
    assert_eq!(answer.order_key, answer_before.order_key);
    let (closed_certification, closed_binding) = {
        let recent = owner.recent.lock().unwrap();
        let slot = recent
            .slots
            .iter()
            .find(|slot| slot.admission == receipt.0)
            .unwrap();
        (
            slot.certification,
            serde_json::to_value(slot.binding.as_ref().unwrap()).unwrap(),
        )
    };
    let closed_head = head("agent-1").unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}",
        json!({"type":"event_msg","payload":{"type":"agent_message","turn_id":"unproven-next","message":"Synthetic later unqualified output."}}))
        .unwrap();
    let extended = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(completed.revision.clone()),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert_eq!(head("agent-1").unwrap(), closed_head);
    let later = extended
        .events
        .iter()
        .find(|event| event.text.as_deref() == Some("Synthetic later unqualified output."))
        .unwrap();
    assert!(value(&extended)
        .legacy_turns
        .iter()
        .any(|turn| turn.event_refs.contains(&later.id)));
    {
        let recent = owner.recent.lock().unwrap();
        let slot = recent
            .slots
            .iter()
            .find(|slot| slot.admission == receipt.0)
            .unwrap();
        let next_binding = serde_json::to_value(slot.binding.as_ref().unwrap()).unwrap();
        assert_eq!(next_binding["presentation"], closed_binding["presentation"]);
        assert_eq!(next_binding["owners"], closed_binding["owners"]);
        assert!(slot.certification > closed_certification,
            "a newer source snapshot must fence stale publication even when only unqualified rows changed");
        assert!(!slot.certification_matches(closed_certification, true));
        assert_eq!(slot.issued, receipt.1);
    }
    fs::rename(&path, temp.path().join("replaced-source.jsonl")).unwrap();
    fs::write(&path, contents).unwrap();
    let retracted = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(completed.revision),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert!(retracted.reset && retracted.presentation.is_none());
    assert!(retired_view::split(&retracted.revision).is_some());
    assert!(
        retracted
            .events
            .iter()
            .any(|event| event.text.as_deref() == Some("Synthetic answer waiting for closure.")),
        "forced retraction must immediately show the available partial source through legacy"
    );
    assert!(!owner
        .publish_recent(&source, &runtime, &receipt, Vec::new())
        .unwrap());
}

#[test]
fn native_source_index_generation_preserves_canonical_preview_binding_and_provenance() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("native-generation-preview.jsonl");
    let header = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"native-generation-session\"}}\n";
    let mut lines = vec![
        json!({"type":"turn_context","payload":{"turn_id":"native-generation-turn"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"native-generation-turn"}}),
        json!({"type":"response_item","payload":{"type":"message","id":"native-generation-prompt","role":"user","content":[{"type":"input_text","text":"Run synthetic tests."}],"internal_chat_message_metadata_passthrough":{"turn_id":"native-generation-turn","content_item_kinds":["user.text"]}}}),
    ];
    for index in 0..12 {
        lines.push(json!({"type":"event_msg","payload":{"type":"agent_message","turn_id":"native-generation-turn","message":format!("Synthetic commentary {index}.")}}));
    }
    lines.push(json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"x".repeat(950)}],"internal_chat_message_metadata_passthrough":{"turn_id":"native-generation-turn"}}}));
    lines.push(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"native-generation-turn"}}));
    let contents = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let (mut source, _) = super::tests::owned_native_source_for_provider_session(
        &path,
        header,
        "agent-1",
        "native-generation-session",
    );
    source.current_status = "Idle".into();
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    let archive = ConversationArchiveState::default();
    let mut prime = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut prime.events, "codex", &path);
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
        Some(prime.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &batch.events,
            Some(&prime.next),
            &batch.next,
            batch.source_proof(),
        )
        .unwrap();
    super::tests::finish_projection(&archive);
    let canonical_ref = head("agent-1").unwrap().unwrap();
    let canonical: Head = store("agent-1").unwrap().read(&canonical_ref).unwrap();
    let mut canonical_reader = store("agent-1").unwrap();
    let (canonical_rows, _) =
        page(&mut canonical_reader, &canonical_ref, &canonical, None).unwrap();
    let captured = canonical
        .turns
        .page(&mut canonical_reader, &canonical_rows, false)
        .unwrap()
        .expect("canonical capture has prepared turn metadata");
    assert!(!captured.items.is_empty());
    assert_eq!(captured.turns.len(), 1);
    assert!(captured.turns[0].membership_complete);
    assert_eq!(captured.turns[0].status, ChatTurnStatus::Completed);
    assert!(captured
        .items
        .iter()
        .all(|item| item.provenance == ChatItemProvenance::Canonical));
    let root = batch
        .events
        .iter()
        .find(|event| {
            event.role == Some(AgentChatRole::User) && event.metadata["input_purpose"] == "request"
        })
        .and_then(|event| event.metadata["request_root_id"].as_str())
        .expect("owned appended prompt establishes its request root");
    let closed = canonical
        .turns
        .closed_roots(
            &mut canonical_reader,
            &super::super::chat_turn_index::Scope {
                agent_id: &canonical.agent_id,
                conversation_id: &canonical.conversation_id,
                source_key: canonical.source_key.as_deref(),
                source_epoch: canonical.source_epoch.as_deref(),
                can_activate: false,
            },
            &BTreeSet::from([root.to_owned()]),
        )
        .unwrap();
    assert!(
        closed.contains_key(root),
        "captured turn is proven closed before binding"
    );
    let owner = &archive.chat_projection;
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    assert!(owner
        .publish_recent(&source, &runtime, &receipt, Vec::new())
        .unwrap());
    let bound =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    let original = value(&bound);
    assert_eq!(
        original.preview_bridge.as_ref().unwrap().admission_id,
        receipt.0
    );
    assert!(!original.items.is_empty());
    assert!(original.items.iter().all(|item| captured
        .items
        .iter()
        .any(|owned| owned.item_key == item.item_key)));
    let original_turns: Vec<_> = original
        .turns
        .iter()
        .map(|turn| turn.turn_key.clone())
        .collect();
    let original_items: Vec<_> = original
        .items
        .iter()
        .map(|item| item.item_key.clone())
        .collect();

    let mut completed_native = None;
    for _ in 0..128 {
        advance_source_index(&source).unwrap();
        if let Some(reference) = super::super::chat_source_index::pointer("agent-1").unwrap() {
            let mut reader = store("agent-1").unwrap();
            let (generation, epoch, _, complete) =
                super::super::chat_source_index::scope_with_completion(
                    &source,
                    &mut reader,
                    &reference,
                )
                .unwrap();
            if complete {
                completed_native = Some((generation, epoch));
                break;
            }
        }
    }
    let (native_generation, native_epoch) =
        completed_native.expect("native source index completes within bounded checkpoints");
    assert_ne!(native_generation, canonical.generation);
    assert_eq!(
        canonical.source_epoch.as_deref(),
        Some(native_epoch.as_str())
    );
    assert_eq!(
        head("agent-1").unwrap().as_deref(),
        Some(canonical_ref.as_str())
    );
    let staged = read_preview_source(&context, &source, PAGE_ROWS).unwrap();
    let ordinary =
        read_with_revalidation_and_limit(&context, &source, None, None, None, None, Some(8))
            .unwrap();
    assert_eq!(
        staged.generation.as_deref(),
        Some(native_generation.as_str())
    );
    assert_eq!(
        ordinary.generation.as_deref(),
        Some(native_generation.as_str())
    );
    assert!(
        !ordinary.archive_read_ready
            && ordinary.detail.is_none()
            && ordinary.revalidated_ids.is_none()
    );
    let mut failures = Vec::new();
    // Inspect a fresh open first: a broken bound repeat must not erase its A
    // before the independent no-revision case can be observed.
    for (label, revision) in [("fresh-open", None), ("bound-repeat", Some(bound.revision))] {
        match owner.recent_reader_with_hints(
            source.clone(),
            runtime.clone(),
            revision,
            8,
            Some(Vec::new()),
        )() {
            Ok(Some(page)) => {
                let decoded = page
                    .presentation
                    .as_ref()
                    .and_then(|value| value.decode_v1().unwrap());
                if let Some(decoded) = decoded.filter(|value| value.preview_bridge.is_some()) {
                    let bridge = decoded.preview_bridge.as_ref().unwrap();
                    assert_eq!(bridge.admission_id, receipt.0);
                    assert_eq!(bridge.canonical_head, canonical_ref);
                    assert_eq!(
                        page.conversation_id.as_deref(),
                        Some(canonical.conversation_id.as_str())
                    );
                    assert_eq!(
                        page.generation.as_deref(),
                        Some(canonical.generation.as_str())
                    );
                    assert_eq!(page.source_epoch, canonical.source_epoch);
                    assert_eq!(
                        decoded
                            .turns
                            .iter()
                            .map(|turn| turn.turn_key.clone())
                            .collect::<Vec<_>>(),
                        original_turns
                    );
                    assert_eq!(
                        decoded
                            .items
                            .iter()
                            .map(|item| item.item_key.clone())
                            .collect::<Vec<_>>(),
                        original_items
                    );
                    assert!(
                        decoded
                            .items
                            .iter()
                            .all(|item| item.provenance == ChatItemProvenance::Canonical),
                        "canonically captured rows must not regress to provisional provenance"
                    );
                    assert!(!page.archive_read_ready && page.next_before.is_none() && page.detail.is_none()
                    && page.revalidated_ids.is_none() && decoded.detail.is_none(),
                    "native generation cannot grant canonical read, paging, detail or ACK authority");
                } else {
                    failures.push(format!(
                        "{label}: no bridge; progress={} reset={} generation={:?}",
                        page.progress, page.reset, page.generation
                    ));
                }
            }
            Ok(None) => failures.push(format!("{label}: no qualified page")),
            Err(error) => failures.push(format!("{label}: {:?}: {error}", error.kind())),
        }
    }
    assert_eq!(
        head("agent-1").unwrap().as_deref(),
        Some(canonical_ref.as_str())
    );
    assert!(
        failures.is_empty(),
        "native generation must preserve canonical binding: {}",
        failures.join("; ")
    );
}

#[test]
fn passive_bound_recent_refresh_keeps_empty_continuity_for_unchanged_and_unqualified_tail() {
    let mut missing = Vec::new();
    for append_tail in [false, true] {
        let (_guard, temp) = isolated_home();
        let path = temp.path().join("passive-continuity.jsonl");
        let contents = [
        json!({"type":"session_meta","payload":{"id":"continuity-session"}}),
        json!({"type":"turn_context","payload":{"turn_id":"closed-turn"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"closed-turn"}}),
        json!({"type":"response_item","payload":{"type":"message","id":"continuity-prompt","role":"user","content":[{"type":"input_text","text":"Run synthetic tests."}],"internal_chat_message_metadata_passthrough":{"turn_id":"closed-turn","content_item_kinds":["user.text"]}}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Synthetic closed answer."}],"internal_chat_message_metadata_passthrough":{"turn_id":"closed-turn"}}}),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"closed-turn"}}),
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n";
        let (mut source, _) = super::tests::owned_native_source_for_provider_session(
            &path,
            &contents,
            "agent-1",
            "continuity-session",
        );
        source.current_status = "Idle".into();
        let context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
        let archive = ConversationArchiveState::default();
        let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            context.provider_source_key.as_deref().unwrap(),
            None,
            true,
        )
        .unwrap();
        crate::commands::chat::decorate_forward_provider_log_events(
            &mut batch.events,
            "codex",
            &path,
        );
        archive
            .append_verified_provider_log_batch_with_context(
                context,
                &batch.events,
                None,
                &batch.next,
                batch.source_proof(),
            )
            .unwrap();
        super::tests::finish_projection(&archive);
        let owner = &archive.chat_projection;
        let runtime = Arc::new(Mutex::new("Idle".into()));
        let receipt = owner.begin_recent(&source, &runtime).unwrap();
        assert!(owner
            .publish_recent(&source, &runtime, &receipt, Vec::new())
            .unwrap());
        let bound = owner.recent_reader_with_hints(
            source.clone(),
            runtime.clone(),
            None,
            8,
            Some(Vec::new()),
        )()
        .unwrap()
        .unwrap();
        assert_eq!(value(&bound).turns[0].status, ChatTurnStatus::Completed);
        assert_eq!(
            value(&bound).preview_bridge.unwrap().admission_id,
            receipt.0
        );
        let request = |revision| RetiredChatRead {
            cursor: None,
            revision: Some(revision),
            detail_ref: None,
            revalidate_ids: None,
            limit: Some(8),
        };
        let first = owner
            .canonical_bridge_reader(
                source.clone(),
                runtime.clone(),
                request(bound.revision.clone()),
            )
            .unwrap()()
        .unwrap();
        let current_head = head("agent-1").unwrap().unwrap();
        if append_tail {
            writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}",
            json!({"type":"event_msg","payload":{"type":"agent_message","turn_id":"unproven-next","message":"Synthetic passive unqualified output."}})).unwrap();
        }
        let continued = owner
            .canonical_bridge_reader(
                source.clone(),
                runtime.clone(),
                request(first.revision.clone()),
            )
            .unwrap()()
        .unwrap();
        assert_eq!(
            head("agent-1").unwrap().as_deref(),
            Some(current_head.as_str())
        );
        if !append_tail {
            assert!(continued.unchanged && continued.events.is_empty());
        }
        let envelope = continued
            .presentation
            .as_ref()
            .and_then(|value| value.decode_v1().unwrap())
            .filter(|value| value.preview_bridge.is_some());
        if let Some(envelope) = envelope {
            assert!(
                envelope.turns.is_empty()
                    && envelope.items.is_empty()
                    && envelope.state_event_refs.is_empty()
                    && envelope.legacy_turns.is_empty()
                    && envelope.detail.is_none()
            );
            let bridge = envelope.preview_bridge.unwrap();
            assert_eq!(bridge.admission_id, receipt.0);
            assert_eq!(bridge.canonical_head, current_head);
            assert_eq!(continued.conversation_id, bound.conversation_id);
            assert_eq!(continued.generation, bound.generation);
            assert_eq!(continued.source_epoch, bound.source_epoch);
            assert!(
                continued.events.is_empty()
                    && continued.aliases.is_empty()
                    && continued.removed_ids.is_empty()
                    && continued.next_before.is_none()
                    && continued.detail.is_none()
                    && continued.revalidated_ids.is_none()
                    && !continued.archive_read_ready
                    && !continued.reset
                    && !continued.older_page
            );
        } else {
            missing.push(format!(
                "append_tail={append_tail}: unchanged={}, events={}, bridge absent",
                continued.unchanged,
                continued.events.len()
            ));
        }
        if append_tail {
            let active = owner.recent_reader_with_hints(
                source.clone(),
                runtime.clone(),
                Some(bound.revision),
                8,
                Some(Vec::new()),
            )()
            .unwrap()
            .unwrap();
            assert!(active.events.iter().any(
                |event| event.text.as_deref() == Some("Synthetic passive unqualified output.")
            ));
            assert_eq!(
                value(&active).preview_bridge.unwrap().admission_id,
                receipt.0
            );
        }
        let pending = owner
            .canonical_bridge_reader(source.clone(), runtime.clone(), request(continued.revision))
            .unwrap();
        let _fence = owner.recent_boundary(Some("agent-1")).unwrap();
        let retired = pending().unwrap();
        if retired.presentation.is_some()
            || !retired.events.is_empty()
            || retired_view::split(&retired.revision).is_none()
        {
            missing.push(format!(
                "append_tail={append_tail}: privacy did not retire continuity"
            ));
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("; "));
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

#[test]
fn literal_preview_hints_are_per_reader_and_unknown_selectors_keep_omitted_behavior() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let mut candidate = candidate(turn_events(Some("Synthetic success")));
    candidate.context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    owner.committed(candidate, "hint-source".into()).unwrap();
    settle(&owner);
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    owner
        .publish_recent(
            &source,
            &runtime,
            &receipt,
            vec![event("watch-unproven", "Recent activity")],
        )
        .unwrap();
    let omitted = owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, None)()
        .unwrap()
        .unwrap();
    for hint in [
        "unknown",
        "source:foreign-agent:foreign-scope:1",
        "stale-preview-token",
    ] {
        let page = owner.recent_reader_with_hints(
            source.clone(),
            runtime.clone(),
            None,
            8,
            Some(vec![hint.into()]),
        )()
        .unwrap()
        .unwrap();
        assert_eq!(
            serde_json::to_value(page).unwrap(),
            serde_json::to_value(&omitted).unwrap(),
            "unresolved hints must expose the omitted response"
        );
    }
    let desktop = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some("legacy-desktop".into()),
        8,
        Some(vec!["synthetic-prompt".into()]),
    )()
    .unwrap()
    .unwrap();
    let remote = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some("legacy-remote".into()),
        8,
        Some(vec!["synthetic-answer".into()]),
    )()
    .unwrap()
    .unwrap();
    assert_eq!(
        value(&desktop).preview_bridge.unwrap().admission_id,
        receipt.0
    );
    assert_eq!(
        value(&remote).preview_bridge.unwrap().admission_id,
        receipt.0
    );
    assert!(desktop
        .aliases
        .iter()
        .any(|alias| alias.observation_id == "synthetic-prompt"));
    assert!(remote
        .aliases
        .iter()
        .any(|alias| alias.observation_id == "synthetic-answer"));
    assert!(!remote
        .aliases
        .iter()
        .any(|alias| alias.observation_id == "watch-unproven"));
    let live = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(desktop.revision.clone()),
        8,
        Some(vec!["uncaptured-live-item".into()]),
    )()
    .unwrap()
    .unwrap();
    assert_eq!(value(&live).preview_bridge.unwrap().admission_id, receipt.0);
    // Older clients still receive their exact existing preview after another
    // reader has acquired the producer binding.
    let after = owner.recent_reader_with_hints(source.clone(), runtime, None, 8, None)()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(omitted).unwrap()
    );
}

#[test]
fn a_full_recent_window_of_canonical_turns_fits_the_existing_reader_budget() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let events: Vec<_> = (0..40)
        .flat_map(|turn| {
            turn_events(Some("Synthetic output"))
                .into_iter()
                .map(move |mut event| {
                    event.id = format!("turn-{turn}-{}", event.id);
                    event.sequence = event.sequence.map(|sequence| turn * 10 + sequence);
                    event.metadata["request_root_id"] = json!(format!("request-{turn}"));
                    if event.turn_id.is_some() {
                        event.turn_id = Some(format!("call-{turn}"));
                    }
                    event
                })
        })
        .collect();
    let mut candidate = candidate(events);
    candidate.context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    owner.committed(candidate, "full-window".into()).unwrap();
    settle(&owner);
    // The ordinary canonical page may win the race before Watch publishes an
    // admission. An aware reader already using this renderer can join later.
    let initial = read_with_revalidation_and_limit(
        &crate::commands::chat::conversation_archive_context_from_snapshot(&source),
        &source,
        None,
        None,
        None,
        None,
        Some(PAGE_ROWS),
    )
    .unwrap();
    assert!(value(&initial).preview_bridge.is_none());
    let receipt = owner.begin_recent(&source, &runtime).unwrap();
    owner
        .publish_recent(&source, &runtime, &receipt, vec![])
        .unwrap();
    let page = owner.recent_reader_with_hints(
        source,
        runtime,
        Some(initial.revision),
        PAGE_ROWS,
        Some(Vec::new()),
    )()
    .unwrap()
    .expect("a proven full window must stay inside the read budget");
    assert!(value(&page).turns.len() >= 8);
    assert!(page.events.len() <= PAGE_ROWS);
    assert!(page.records_decoded <= super::super::chat_read_store::READ_OBJECTS);
    assert!(page.bytes_read < 2 * super::super::chat_read_store::READ_BYTES);
}

#[test]
fn preview_hint_validation_is_whole_request_and_uses_utf8_bytes() {
    use crate::commands::chat::validate_preview_backing_ids as validate;
    assert!(validate(Some(&[]), true, None, None, false).is_ok());
    assert!(validate(Some(&vec!["x".repeat(512); 8]), true, None, None, false).is_ok());
    for ids in [
        vec!["x".into(); 9],
        vec!["x".repeat(513)],
        vec!["é".repeat(257)],
        vec![" ".into()],
    ] {
        assert_eq!(
            validate(Some(&ids), true, None, None, false).unwrap_err(),
            "invalid chat read request"
        );
    }
    for (recent, cursor, detail, ack) in [
        (false, None, None, false),
        (true, Some("older"), None, false),
        (true, None, Some("detail"), false),
        (true, None, None, true),
    ] {
        assert!(validate(Some(&[]), recent, cursor, detail, ack).is_err());
    }
}

#[test]
fn retired_admission_can_requalify_under_new_a_with_exact_returned_hint_aliases() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let mut candidate = candidate(turn_events(Some("Synthetic output")));
    candidate.context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    owner
        .committed(candidate, "sticky-retraction".into())
        .unwrap();
    settle(&owner);
    let first = owner.begin_recent(&source, &runtime).unwrap();
    owner
        .publish_recent(&source, &runtime, &first, vec![])
        .unwrap();
    let bound =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    assert!(value(&bound).preview_bridge.is_some());
    let fence = owner.recent_boundary(Some("agent-1")).unwrap();
    let retired = owner.recent_reader_with_hints(
        source.clone(),
        runtime.clone(),
        Some(bound.revision),
        8,
        Some(Vec::new()),
    )()
    .unwrap()
    .unwrap();
    assert!(retired.presentation.is_none() && retired.reset);
    assert_eq!(retired_view::split(&retired.revision).unwrap().1, first.0);
    drop(fence);
    let second = owner.begin_recent(&source, &runtime).unwrap();
    assert_ne!(first.0, second.0);
    let request = RetiredChatRead {
        cursor: None,
        revision: Some(retired.revision),
        detail_ref: None,
        revalidate_ids: None,
        limit: Some(8),
    };
    let unpublished = owner
        .retired_view_reader(
            source.clone(),
            runtime.clone(),
            request.clone(),
            Some(Vec::new()),
        )
        .unwrap()()
    .unwrap();
    assert!(unpublished.presentation.is_none());
    assert_eq!(
        retired_view::split(&unpublished.revision).unwrap().1,
        first.0
    );
    owner
        .publish_recent(&source, &runtime, &second, vec![])
        .unwrap();
    // Omitted hints preserve the ordinary legacy path even with new A ready.
    let legacy = owner
        .retired_view_reader(source.clone(), runtime.clone(), request.clone(), None)
        .unwrap()()
    .unwrap();
    assert!(legacy.presentation.is_none() && !legacy.events.is_empty());
    assert_eq!(retired_view::split(&legacy.revision).unwrap().1, first.0);
    let hints: Vec<_> = legacy.events.iter().map(|event| event.id.clone()).collect();
    assert!(hints.len() <= 8);
    let unknown = owner
        .retired_view_reader(
            source.clone(),
            runtime.clone(),
            request.clone(),
            Some(vec!["unknown-literal-selector".into()]),
        )
        .unwrap()()
    .unwrap();
    assert_eq!(
        serde_json::to_value(&unknown).unwrap(),
        serde_json::to_value(&legacy).unwrap()
    );
    let wrong_runtime = owner
        .retired_view_reader(
            source.clone(),
            Arc::new(Mutex::new("Idle".into())),
            request.clone(),
            Some(hints.clone()),
        )
        .unwrap()()
    .unwrap();
    assert!(wrong_runtime.presentation.is_none());
    assert_eq!(
        retired_view::split(&wrong_runtime.revision).unwrap().1,
        first.0
    );
    let rebound = owner
        .retired_view_reader(
            source.clone(),
            runtime.clone(),
            request.clone(),
            Some(hints.clone()),
        )
        .unwrap()()
    .unwrap();
    assert_eq!(
        value(&rebound).preview_bridge.unwrap().admission_id,
        second.0
    );
    assert_eq!(
        preview_bridge::split_revision(&rebound.revision).unwrap().1,
        second.0
    );
    assert!(retired_view::split(&rebound.revision).is_none());
    assert!(hints.iter().all(|id| rebound
        .aliases
        .iter()
        .any(|alias| &alias.observation_id == id)));
    assert!(!owner
        .publish_recent(&source, &runtime, &first, vec![])
        .unwrap());

    // Work captured before a new privacy fence cannot consume the new A.
    let pending = owner
        .retired_view_reader(source.clone(), runtime.clone(), request, Some(hints))
        .unwrap();
    let _fence = owner.recent_boundary(Some("agent-1")).unwrap();
    let blocked = pending().unwrap();
    assert!(blocked.presentation.is_none() && blocked.events.is_empty() && blocked.reset);
    assert_eq!(retired_view::split(&blocked.revision).unwrap().1, first.0);
    assert!(!owner
        .publish_recent(&source, &runtime, &second, vec![])
        .unwrap());
}

#[test]
fn retired_admission_stays_legacy_while_same_a_is_still_live() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let source = snapshot(&context());
    let runtime = Arc::new(Mutex::new("Idle".into()));
    let mut candidate = candidate(turn_events(Some("Synthetic output")));
    candidate.context = crate::commands::chat::conversation_archive_context_from_snapshot(&source);
    owner
        .committed(candidate, "same-admission-retraction".into())
        .unwrap();
    settle(&owner);
    let admission = owner.begin_recent(&source, &runtime).unwrap();
    owner
        .publish_recent(&source, &runtime, &admission, vec![])
        .unwrap();
    let bound =
        owner.recent_reader_with_hints(source.clone(), runtime.clone(), None, 8, Some(Vec::new()))(
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        value(&bound).preview_bridge.unwrap().admission_id,
        admission.0
    );
    let request = RetiredChatRead {
        cursor: None,
        revision: Some(format!("recent:{}:retired", admission.0)),
        detail_ref: None,
        revalidate_ids: None,
        limit: Some(8),
    };
    let legacy = owner
        .retired_view_reader(source.clone(), runtime.clone(), request.clone(), None)
        .unwrap()()
    .unwrap();
    assert!(!legacy.events.is_empty());
    let hints = legacy.events.iter().map(|event| event.id.clone()).collect();
    for hints in [Some(Vec::new()), Some(hints)] {
        let same = owner
            .retired_view_reader(source.clone(), runtime.clone(), request.clone(), hints)
            .unwrap()()
        .unwrap();
        assert!(same.presentation.is_none());
        assert!(same.recent_admission.is_none());
        assert_eq!(retired_view::split(&same.revision).unwrap().1, admission.0);
    }
    assert!(owner
        .recent
        .lock()
        .unwrap()
        .slots
        .iter()
        .any(|slot| { slot.admission == admission.0 && slot.published && slot.binding.is_some() }));
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
