use super::*;
use crate::state::conversation_archive::{tests::isolated_home, ConversationArchiveState};
use wardian_core::conversations::AgentConversationLoggingSetting;
use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole};

#[test]
fn display_headers_preserve_all_bounded_status_variants() {
    use wardian_core::models::chat::AgentChatStatus;
    for status in [
        None,
        Some(AgentChatStatus::Running),
        Some(AgentChatStatus::Succeeded),
        Some(AgentChatStatus::Failed),
        Some(AgentChatStatus::ActionRequired),
        Some(AgentChatStatus::Cancelled),
        Some(AgentChatStatus::Idle),
        Some(AgentChatStatus::Processing),
        Some(AgentChatStatus::Unknown),
    ] {
        let mut original = event("status", "");
        original.status = status;
        assert_eq!(header(&original).status, original.status);
    }
}

#[test]
fn body_artifact_basenames_use_the_production_archive_materializer() {
    let (_guard, _temp) = isolated_home();
    let directory = super::super::conversation_dir("agent-1", "conversation").unwrap();
    let text = "x".repeat(wardian_core::conversations::CONVERSATION_INLINE_TEXT_LIMIT_BYTES + 1);
    let payload = wardian_core::conversations::materialize_text_payload(
        &directory.join("artifacts"),
        "body",
        &text,
    )
    .unwrap();
    assert_eq!(Path::new(&payload.artifact_refs[0]).components().count(), 1);
    let mut row = event("materialized", "");
    row.text = payload.text;
    row.metadata =
        json!({"text_excerpt": payload.excerpt, "text_artifact_refs": payload.artifact_refs});
    let mut source = body_source(&candidate(vec![]), &row).unwrap().unwrap();
    assert_eq!(source.length().unwrap(), text.len() as u64);
    let chunk = source.chunk(0).unwrap();
    assert_eq!(chunk, text.as_bytes()[..text.len().min(BODY_BYTES)]);
}

#[test]
fn unknown_delivered_input_remains_unresolved_after_native_provider_adoption() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_delivered_input("agent-1", "same prompt", Some("peer"))
        .unwrap();
    let mut native = event("internal", "same prompt");
    native.provider = "antigravity".into();
    native.metadata = json!({"provider_log": true, "input_origin": "provider_internal", "input_purpose": "internal", "provider_step_source": 2});
    archive.append_chat_events("agent-1", &[native]).unwrap();
    let archived = archive.chat_events_for_agent("agent-1").unwrap();
    let canonical_input = archived
        .iter()
        .find(|row| row.metadata["generated"] == true)
        .unwrap();
    let context = ConversationArchiveContext::for_agent_id("agent-1", "antigravity");
    let page = read(&context, &snapshot(&context), None, None, None).unwrap();
    let input = page
        .events
        .iter()
        .find(|row| row.metadata["generated"] == true)
        .unwrap();
    assert_eq!(input.provider, "unknown");
    assert_eq!(input.metadata["input_origin"], "agent_input");
    assert_eq!(input.metadata["chat_identity_resolution"], "unresolved");
    assert!(input.metadata.get("chat_source_ref").is_none());
    assert!(page.aliases.is_empty());
    let directory =
        super::super::conversation_dir("agent-1", page.conversation_id.as_deref().unwrap())
            .unwrap();
    let records = super::super::read_jsonl_records(&directory.join("conversation.jsonl")).unwrap();
    let mut candidate = candidate(vec![]);
    candidate.conversation_id = page.conversation_id.unwrap();
    candidate.generated_ids.insert(canonical_input.id.clone());
    candidate.generated_input_bindings =
        generated_input_bindings(&records, &candidate.conversation_id);
    assert!(is_owned_unknown_input(&candidate, canonical_input));
    let mut forged = canonical_input.clone();
    forged.session_id = "other-agent".into();
    assert!(!is_owned_unknown_input(&candidate, &forged));
    forged = canonical_input.clone();
    forged.metadata["archive_record"]["request_root_id"] = json!("unrelated-request");
    assert!(!is_owned_unknown_input(&candidate, &forged));
    candidate.generated_input_bindings.clear();
    assert!(!is_owned_unknown_input(&candidate, canonical_input));
}

#[test]
fn foreign_provider_display_filter_preserves_archive_and_agent_ownership() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut foreign = event("foreign-provider", "legacy provider record");
    foreign.provider = "claude".into();
    assert_eq!(
        archive
            .append_chat_events_with_context(context(), &[foreign.clone()])
            .unwrap(),
        1
    );
    let id = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let directory = super::super::conversation_dir("agent-1", &id).unwrap();
    let persisted: Vec<AgentChatEvent> =
        super::super::read_jsonl_records(&directory.join("events.jsonl")).unwrap();
    assert_eq!(persisted, vec![foreign.clone()]);
    let page = read(&context(), &snapshot(&context()), None, None, None).unwrap();
    assert!(page.events.is_empty());
    assert!(page.aliases.is_empty());
    foreign.session_id = "other-agent".into();
    assert!(archive
        .append_chat_events_with_context(context(), &[foreign])
        .is_err());
    assert_eq!(archive.chat_events_for_agent("agent-1").unwrap().len(), 1);
}

#[test]
fn known_projection_extent_is_scoped_and_survives_missing_archive_files() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let mut committed = candidate(vec![event("recorded", "retained")]);
    committed.committed_output_bytes = 333;
    owner.committed(committed, "one".into()).unwrap();
    let directory = super::super::conversation_dir("agent-1", "conversation").unwrap();
    assert!(!directory.join("events.jsonl").exists());
    assert_eq!(
        committed_output_extent("agent-1", "conversation").unwrap(),
        Some(333)
    );
    assert_eq!(
        committed_output_extent("agent-1", "new-conversation").unwrap(),
        None
    );
}

#[test]
fn body_artifacts_reject_paths_outside_the_owned_artifacts_directory() {
    let (_guard, temp) = isolated_home();
    let outside = temp.path().join("outside.txt");
    fs::write(&outside, "private body").unwrap();
    for reference in [
        "".to_owned(),
        "../outside.txt".to_owned(),
        "artifacts/../../outside.txt".to_owned(),
        "other/body.txt".to_owned(),
        "artifacts/./body.txt".to_owned(),
        "file:body.txt".to_owned(),
        "body.txt:private".to_owned(),
        outside.to_string_lossy().into_owned(),
    ] {
        let mut row = event("unsafe", "");
        row.text = None;
        row.metadata = json!({"text_artifact_refs": [reference]});
        assert!(body_source(&candidate(vec![]), &row).is_err());
    }
}

#[test]
fn production_tool_parsers_preserve_the_shared_header_and_lazy_body_contract() {
    let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../src/features/chat/chatToolProjectionFixture.json"
    ))
    .unwrap();
    for fixture in fixtures {
        let original = crate::providers::chat_transcript::normalize_chat_line(
            "agent-1",
            fixture["provider"].as_str().unwrap(),
            &serde_json::to_string(&fixture["raw"]).unwrap(),
            1,
        )
        .unwrap();
        let projected = header(&original);
        for (key, value) in fixture["metadata"].as_object().unwrap() {
            assert_eq!(
                &projected.metadata[key], value,
                "{} {key}",
                fixture["title"]
            );
        }
        assert_eq!(
            projected.metadata["chat_tool_input_truncated"],
            serde_json::Value::Null
        );
        let mut body = body_source(&candidate(vec![]), &original).unwrap().unwrap();
        let mut offset = 0;
        let mut bytes = Vec::new();
        while offset < body.length().unwrap() {
            let chunk = body.chunk(offset).unwrap();
            assert!(!chunk.is_empty() && chunk.len() <= BODY_BYTES);
            offset += chunk.len() as u64;
            bytes.extend(chunk);
        }
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            fixture["detail_text"].as_str().unwrap()
        );
    }
}

#[test]
fn structured_preview_is_bounded_but_counts_and_lazy_input_cover_the_original() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let input = json!({"file_path": "src/large.ts", "content": "雪\r\n".repeat(5000), "private_extra": "x".repeat(5000)});
    let mut tool = event("write", "");
    tool.kind = AgentChatEventKind::ToolCall;
    tool.text = None;
    tool.metadata = json!({"tool_name": "Write", "tool_input": input, "files_written": ["src/large.ts"], "unused": "x".repeat(10000)});
    let projected = header(&tool);
    assert!(
        serde_json::to_vec(&projected.metadata["tool_input"])
            .unwrap()
            .len()
            <= 2048
    );
    assert_eq!(
        projected.metadata["chat_edit_summary"],
        json!({"kind": "write", "added": 5000, "removed": 0})
    );
    assert_eq!(projected.metadata["chat_tool_input_truncated"], true);
    assert!(projected.metadata.get("unused").is_none());
    let expected = tool_input_body(&tool).unwrap();
    owner
        .committed(candidate(vec![tool]), "tool-input".into())
        .unwrap();
    while owner.advance("agent-1").unwrap() {}
    let ctx = context();
    let snap = snapshot(&ctx);
    let page = read(&ctx, &snap, None, None, None).unwrap();
    let mut next = page.events[0].metadata["chat_detail_ref"]
        .as_str()
        .map(str::to_owned);
    let mut actual = String::new();
    while let Some(reference) = next {
        let detail = read(&ctx, &snap, None, None, Some(&reference))
            .unwrap()
            .detail
            .unwrap();
        assert!(detail.text.len() <= BODY_BYTES);
        actual.push_str(&detail.text);
        next = detail.next;
    }
    assert_eq!(actual, expected);
}

#[test]
fn native_metadata_only_patch_gets_a_lazy_detail_reference() {
    let (_guard, temp) = isolated_home();
    let patch = format!(
        "*** Begin Patch\n*** Add File: src/large.ts\n{}*** End Patch",
        "+雪\n".repeat(2000)
    );
    let contents = format!(
        "{}\n{}\n",
        json!({"type": "session_meta", "payload": {"id": "one"}}),
        json!({"type": "response_item", "payload": {"type": "custom_tool_call", "name": "apply_patch", "input": patch}})
    );
    assert!(contents.len() <= 2 * 16 * 1024);
    let (snapshot, _) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let page = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let event = page
        .events
        .iter()
        .find(|event| event.kind == AgentChatEventKind::ToolCall)
        .unwrap();
    assert!(event.text.is_none());
    assert_eq!(event.metadata["chat_tool_input_truncated"], true);
    let reference = event.metadata["chat_detail_ref"].as_str().unwrap();
    let detail = crate::commands::chat_recent_seed::detail(&snapshot, reference).unwrap();
    assert_eq!(detail.text, patch);
    assert!(detail.complete);
    assert!(
        crate::commands::chat_recent_seed::detail(&snapshot, &format!("{reference}:1")).is_ok()
    );
    assert!(crate::commands::chat_recent_seed::detail(
        &snapshot,
        &format!("{reference}:{}", patch.find('雪').unwrap() + 1)
    )
    .is_err());
    assert!(crate::commands::chat_recent_seed::detail(
        &snapshot,
        &format!("{reference}:{}", patch.len() + 1)
    )
    .is_err());
}

#[test]
fn native_argument_and_justification_details_continue_in_bounded_utf8_chunks() {
    let (_guard, temp) = isolated_home();
    let arguments = json!({"cmd": "test", "justification": "雪".repeat(2800)}).to_string();
    let contents = format!(
        "{}\n{}\n",
        json!({"type": "session_meta", "payload": {"id": "one"}}),
        json!({"type": "response_item", "payload": {"type": "custom_tool_call", "name": "exec_command", "input": arguments}})
    );
    let (snapshot, _) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let page = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let reference = page
        .events
        .iter()
        .find(|event| event.kind == AgentChatEventKind::ToolCall)
        .unwrap()
        .metadata["chat_detail_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = crate::commands::chat_recent_seed::detail(&snapshot, &reference).unwrap();
    assert!(first.text.len() <= BODY_BYTES);
    assert!(!first.complete);
    let last = crate::commands::chat_recent_seed::detail(&snapshot, first.next.as_deref().unwrap())
        .unwrap();
    assert!(last.text.len() <= BODY_BYTES && last.complete && last.next.is_none());
    assert_eq!(
        format!("{}{}", first.text, last.text),
        format!("{arguments}\n\n{}", "雪".repeat(2800))
    );
}

#[test]
fn tool_arguments_and_artifact_output_share_the_bounded_body_without_losing_either() {
    let (_guard, _temp) = isolated_home();
    let directory = super::super::storage::conversation_dir("agent-1", "conversation").unwrap();
    fs::create_dir_all(directory.join("artifacts")).unwrap();
    fs::write(directory.join("artifacts/output.txt"), "雪".repeat(7000)).unwrap();
    let mut tool = event("tool-with-output", "");
    tool.text = None;
    tool.metadata = json!({"tool_input_text": "*** Begin Patch\n*** Update File: src/example.ts\n@@\n-old\n+new\n*** End Patch", "text_artifact_refs": ["artifacts/output.txt"]});
    let mut body = body_source(&candidate(vec![]), &tool).unwrap().unwrap();
    let mut actual = String::new();
    let mut offset = 0;
    while offset < body.length().unwrap() {
        let bytes = body.chunk(offset).unwrap();
        assert!(!bytes.is_empty() && bytes.len() <= BODY_BYTES);
        offset += bytes.len() as u64;
        actual.push_str(std::str::from_utf8(&bytes).unwrap());
    }
    assert_eq!(
        actual,
        format!(
            "{}\n\n{}",
            tool_input_body(&tool).unwrap(),
            "雪".repeat(7000)
        )
    );
}

fn event(id: &str, text: &str) -> AgentChatEvent {
    AgentChatEvent {
        id: id.into(),
        session_id: "agent-1".into(),
        provider: "mock".into(),
        kind: AgentChatEventKind::Message,
        role: Some(AgentChatRole::User),
        text: Some(text.into()),
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
        metadata: json!({}),
    }
}

fn context() -> ConversationArchiveContext {
    let mut context = ConversationArchiveContext::for_agent_id("agent-1", "mock");
    context.provider_source_key = Some("mock:one".into());
    context
}

fn snapshot(
    context: &ConversationArchiveContext,
) -> crate::commands::chat::AgentArchiveCaptureSnapshot {
    crate::commands::chat::AgentArchiveCaptureSnapshot {
        session_id: context.agent_id.clone(),
        provider: context.provider.clone(),
        resume_session: None,
        fresh_provider_session_id: None,
        cleared_provider_sessions: Vec::new(),
        current_status: "Idle".into(),
        last_status_at: None,
        log_path: None,
        agent_name: String::new(),
        agent_class: String::new(),
        workspace: String::new(),
        agent_conversation_logging: AgentConversationLoggingSetting::Default,
        watch_state: std::sync::Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            context.agent_id.clone(),
            32,
            4096,
        ))),
    }
}

fn candidate(events: Vec<AgentChatEvent>) -> Candidate {
    Candidate {
        context: context(),
        conversation_id: "conversation".into(),
        events,
        committed_output_bytes: 0,
        verified_ids: Default::default(),
        generated_ids: Default::default(),
        generated_input_bindings: Default::default(),
        source_epoch: None,
    }
}

fn cold_saved_remove_projection(temp: &Path) {
    let (pointer, objects) = locations("agent-1").unwrap();
    assert!(pointer.starts_with(temp) && objects.starts_with(temp));
    if pointer.exists() {
        fs::remove_file(pointer).unwrap();
    }
    if objects.exists() {
        fs::remove_dir_all(objects).unwrap();
    }
}

#[test]
fn cold_saved_restart_keeps_owned_inputs_and_partial_older_cursor() {
    let (_guard, temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut unknown = context();
    unknown.provider = "unknown".into();
    archive
        .append_delivered_input_with_context(unknown, "owned input", Some("peer"))
        .unwrap();
    let events = (0..97)
        .map(|i| event(&format!("cold-{i}"), "saved reply"))
        .collect::<Vec<_>>();
    archive
        .append_chat_events_with_context(context(), &events)
        .unwrap();
    let original = archive.active_conversation_id("agent-1").unwrap().unwrap();
    cold_saved_remove_projection(temp.path());
    assert!(ConversationArchiveState::default()
        .bootstrap_saved_chat(&context())
        .unwrap());
    let partial = read(&context(), &snapshot(&context()), None, None, None).unwrap();
    assert!(
        !partial.events.is_empty(),
        "recent rows publish before full proof indexing"
    );
    assert!(partial.events.len() <= 24);
    let older_cursor = partial.next_before.unwrap();
    let generation = partial.generation.unwrap();
    let mut ready = false;
    for _ in 0..64 {
        // No owner memory survives between checkpoints.
        if !ConversationArchiveState::default()
            .bootstrap_saved_chat(&context())
            .unwrap()
        {
            ready = true;
            break;
        }
    }
    assert!(ready);
    let latest = read(&context(), &snapshot(&context()), None, None, None).unwrap();
    assert_eq!(latest.conversation_id.as_deref(), Some(original.as_str()));
    assert_eq!(latest.generation.as_deref(), Some(generation.as_str()));
    assert_eq!(latest.events.len(), PAGE_ROWS);
    let older = read(
        &context(),
        &snapshot(&context()),
        Some(&older_cursor),
        None,
        None,
    )
    .unwrap();
    assert!(!older.reset);
    let input = older
        .events
        .iter()
        .find(|row| row.metadata["generated"] == true)
        .unwrap();
    assert_eq!(input.provider, "unknown");
    assert_eq!(input.metadata["input_origin"], "agent_input");
    assert_eq!(input.metadata["chat_identity_resolution"], "unresolved");
    assert!(input.metadata.get("chat_source_ref").is_none());
    assert!(older.aliases.is_empty());
}

#[test]
fn cold_saved_latest_closed_and_foreign_source_cannot_bind() {
    let (_guard, temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(context(), &[event("saved", "saved reply")])
        .unwrap();
    cold_saved_remove_projection(temp.path());
    let mut foreign = context();
    foreign.provider_source_key = Some("mock:foreign".into());
    assert!(!ConversationArchiveState::default()
        .bootstrap_saved_chat(&foreign)
        .unwrap());
    assert!(head("agent-1").unwrap().is_none());
    assert!(!ConversationArchiveState::default()
        .bootstrap_saved_chat(&context())
        .unwrap());
    assert!(head("agent-1").unwrap().is_some());
    archive
        .rollover_agent(
            "agent-1",
            wardian_core::conversations::ConversationBoundaryReason::Clear,
        )
        .unwrap();
    assert!(!ConversationArchiveState::default()
        .bootstrap_saved_chat(&context())
        .unwrap());
    assert!(
        head("agent-1").unwrap().is_none(),
        "closed latest index entry must suppress its older open entries"
    );
}

#[test]
fn cold_saved_changed_extent_retires_partial_generation() {
    use std::io::Write;
    let (_guard, temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let events = (0..50)
        .map(|i| event(&format!("extent-{i}"), "saved reply"))
        .collect::<Vec<_>>();
    archive
        .append_chat_events_with_context(context(), &events)
        .unwrap();
    let original = archive.active_conversation_id("agent-1").unwrap().unwrap();
    cold_saved_remove_projection(temp.path());
    assert!(ConversationArchiveState::default()
        .bootstrap_saved_chat(&context())
        .unwrap());
    let path = super::super::conversation_dir("agent-1", &original)
        .unwrap()
        .join("events.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert!(ConversationArchiveState::default()
        .bootstrap_saved_chat(&context())
        .unwrap());
    assert!(head("agent-1").unwrap().is_none());
    let page = read(&context(), &snapshot(&context()), None, None, None).unwrap();
    assert!(page.conversation_id.is_none() && page.events.is_empty());
}

#[test]
fn cold_scope_missing_old_binding_resets_before_read_and_reselects_after_restart() {
    let (_guard, temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let events = (0..64)
        .map(|i| event(&format!("old-binding-{i}"), "saved reply"))
        .collect::<Vec<_>>();
    archive
        .append_chat_events_with_context(context(), &events)
        .unwrap();
    cold_saved_remove_projection(temp.path());
    assert!(ConversationArchiveState::default()
        .bootstrap_saved_chat(&context())
        .unwrap());
    let first = read(&context(), &snapshot(&context()), None, None, None).unwrap();
    let pointer = head("agent-1").unwrap().unwrap();
    let mut store = Store::writer(&locations("agent-1").unwrap().1);
    let mut legacy: serde_json::Value = store.read(&pointer).unwrap();
    // Only strip the new private binding to reproduce the genuine source19
    // checkpoint format. Its conversation, rows and canonical archive stay real.
    legacy["bootstrap"]
        .as_object_mut()
        .unwrap()
        .remove("selection_scope");
    let legacy_ref = store.put(&legacy).unwrap();
    write_json_atomic(&locations("agent-1").unwrap().0, &legacy_ref).unwrap();
    let rejected = read(
        &context(),
        &snapshot(&context()),
        None,
        Some(&first.revision),
        None,
    )
    .unwrap();
    assert!(
        rejected.reset && !rejected.unchanged,
        "old unbound cold checkpoint must not display rows"
    );
    assert!(rejected.conversation_id.is_none() && rejected.events.is_empty());
    assert_ne!(rejected.revision, first.revision);
    assert!(ConversationArchiveState::default()
        .bootstrap_saved_chat(&context())
        .unwrap());
    assert!(head("agent-1").unwrap().is_none());
    assert!(ConversationArchiveState::default()
        .bootstrap_saved_chat(&context())
        .unwrap());
    let reselected = read(&context(), &snapshot(&context()), None, None, None).unwrap();
    assert_eq!(reselected.conversation_id, first.conversation_id);
    assert_ne!(reselected.generation, first.generation);
}

#[test]
fn recent_headers_publish_before_older_backfill_and_unchanged_poll_decodes_only_head() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let mut publication = candidate(
        (0..150)
            .map(|index| event(&format!("row-{index}"), "preview"))
            .collect(),
    );
    publication.source_epoch = Some("canonical-epoch".into());
    owner.committed(publication, "seal".into()).unwrap();
    let ctx = context();
    let snapshot = snapshot(&ctx);
    let first = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(first.events.len(), PAGE_ROWS);
    assert_eq!(first.events.first().unwrap().id, "row-70");
    assert!(first.next_before.is_some());
    assert!(first.bytes_read <= super::super::chat_read_store::READ_BYTES + HEAD_BYTES as usize);
    let unchanged = read(&ctx, &snapshot, None, Some(&first.revision), None).unwrap();
    assert!(unchanged.unchanged);
    assert!(unchanged.events.is_empty());
    assert!(!unchanged.reset);
    assert!(first.conversation_id.is_some());
    assert!(first.generation.is_some());
    assert_eq!(unchanged.conversation_id, first.conversation_id);
    assert_eq!(unchanged.generation, first.generation);
    assert_eq!(unchanged.source_epoch, first.source_epoch);
    assert_eq!(unchanged.progress, first.progress);
    // Scope safety requires one bounded head decode even on unchanged polls.
    assert_eq!(unchanged.records_decoded, 1);
    owner.advance("agent-1").unwrap();
    let older = read(&ctx, &snapshot, first.next_before.as_deref(), None, None).unwrap();
    assert_eq!(older.events.len(), 70);
    assert_eq!(older.events.first().unwrap().id, "row-0");
    assert!(!older.reset);
}

#[test]
fn unchanged_poll_keeps_absent_scope_and_rejects_an_unreadable_head() {
    let (_guard, _temp) = isolated_home();
    let ctx = context();
    let snap = snapshot(&ctx);
    let first = read(&ctx, &snap, None, None, None).unwrap();
    let unchanged = read(&ctx, &snap, None, Some(&first.revision), None).unwrap();
    assert!(unchanged.unchanged && !unchanged.reset);
    assert!(unchanged.events.is_empty());
    assert!(unchanged.conversation_id.is_none());
    assert!(unchanged.generation.is_none());
    assert!(unchanged.source_epoch.is_none());
    assert_eq!(unchanged.progress, first.progress);

    let owner = ProjectionOwner::default();
    owner
        .committed(candidate(vec![event("row", "preview")]), "seal".into())
        .unwrap();
    let published = read(&ctx, &snap, None, None, None).unwrap();
    let mut foreign = ctx.clone();
    foreign.provider_source_key = Some("mock:foreign".into());
    let rejected = read(
        &foreign,
        &snapshot(&foreign),
        None,
        Some(&published.revision),
        None,
    )
    .unwrap();
    assert!(rejected.reset && !rejected.unchanged);
    assert!(rejected.events.is_empty());
    assert!(rejected.conversation_id.is_none());
    assert!(rejected.generation.is_none());
    let reference = head(&ctx.agent_id).unwrap().unwrap();
    let store = store(&ctx.agent_id).unwrap();
    fs::write(store.dir.join(reference), b"{}").unwrap();
    let rejected = read(&ctx, &snap, None, Some(&published.revision), None).unwrap();
    assert!(rejected.reset && !rejected.unchanged);
    assert!(rejected.events.is_empty());
    assert!(rejected.conversation_id.is_none());
    assert!(rejected.generation.is_none());
}

#[test]
fn an_old_generation_resets_to_only_recent_headers_and_foreign_detail_is_rejected() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(
            candidate(vec![event("old", &"a".repeat(4000))]),
            "old".into(),
        )
        .unwrap();
    let ctx = context();
    let snap = snapshot(&ctx);
    let old = read(&ctx, &snap, None, None, None).unwrap();
    let old_detail = old.events[0].metadata["chat_detail_ref"]
        .as_str()
        .unwrap()
        .to_string();
    let cursor = format!(
        "{}:00000000000000000000",
        old.revision.split('.').next().unwrap()
    );
    owner
        .committed(
            candidate(
                (0..200)
                    .map(|index| event(&format!("new-{index}"), "new"))
                    .collect(),
            ),
            "new".into(),
        )
        .unwrap();
    let reset = read(&ctx, &snap, Some(&cursor), None, None).unwrap();
    assert!(reset.reset);
    assert_eq!(reset.events.len(), PAGE_ROWS);
    assert!(read(&ctx, &snap, None, None, Some(&old_detail)).is_err());
    let mut foreign = ctx.clone();
    foreign.agent_id = "other-agent".into();
    assert!(
        read(&foreign, &snapshot(&foreign), None, None, Some(&old_detail))
            .unwrap()
            .detail
            .is_none()
    );
}

#[test]
fn body_chunks_are_bounded_and_payload_changes_invalidate_old_continuations() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(
            candidate(vec![event("body", &"雪".repeat(20_000))]),
            "body".into(),
        )
        .unwrap();
    let ctx = context();
    let snap = snapshot(&ctx);
    let first = read(&ctx, &snap, None, None, None).unwrap();
    let reference = first.events[0].metadata["chat_detail_ref"]
        .as_str()
        .unwrap();
    let chunk = read(&ctx, &snap, None, None, Some(reference))
        .unwrap()
        .detail
        .unwrap();
    assert!(chunk.text.len() <= BODY_BYTES);
    assert!(chunk.next.is_some());
    owner
        .committed(
            candidate(vec![event("body", &"x".repeat(60_000))]),
            "rewrite".into(),
        )
        .unwrap();
    assert!(read(&ctx, &snap, None, None, chunk.next.as_deref()).is_err());
}

#[test]
fn cursor_failure_preserves_previous_head_and_retry_publishes_the_committed_batch() {
    let (_guard, temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let mut ctx = ConversationArchiveContext::for_agent_id("agent-1", "codex");
    ctx.provider_source_key = Some("codex:session:one".into());
    archive
        .append_delivered_input_with_context(ctx.clone(), "previous", None)
        .unwrap();
    let previous = head("agent-1").unwrap();
    let source = temp.path().join("provider.jsonl");
    fs::write(&source, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"one\"}}\n{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"new reply\"}}\n").unwrap();
    let batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &source,
        "codex:session:one",
        None,
        true,
    )
    .unwrap();
    archive
        .fail_next_chat_cursor_commit
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(archive
        .append_provider_log_batch_with_context(ctx.clone(), &batch.events, None, &batch.next)
        .is_err());
    assert_eq!(head("agent-1").unwrap(), previous);
    assert!(archive
        .provider_log_capture_state("agent-1", "codex:session:one")
        .unwrap()
        .is_none());
    archive
        .append_provider_log_batch_with_context(ctx, &batch.events, None, &batch.next)
        .unwrap();
    assert_ne!(head("agent-1").unwrap(), previous);
}

#[test]
fn checkpoint_row_failure_retries_all_unpublished_rows() {
    checkpoint_rows_retry_after_fault(1);
}

#[test]
fn checkpoint_pointer_failure_retries_all_unpublished_rows() {
    checkpoint_rows_retry_after_fault(3);
}

#[test]
fn checkpoint_node_flush_failure_retries_all_unpublished_rows() {
    checkpoint_rows_retry_after_fault(4);
}

#[test]
fn generated_input_pointer_failure_keeps_the_previous_in_memory_head() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(candidate(vec![event("previous", "old")]), "old".into())
        .unwrap();
    let ctx = context();
    let (head_path, _) = locations(&ctx.agent_id).unwrap();
    let previous_pointer = fs::read(&head_path).unwrap();
    let (before, previous_revision, fence) = {
        let works = owner.work.lock().unwrap();
        let work = &works[&ctx.agent_id];
        (
            serde_json::to_vec(&work.head).unwrap(),
            work.published.clone(),
            InputFence {
                conversation_id: work.head.conversation_id.clone(),
                source_epoch: work.head.source_epoch.clone(),
                policy_generation: None,
            },
        )
    };
    // A real final-pointer rename failure exercises the previous mutation bug
    // without relying on a fault hook introduced by this change.
    let backup = head_path.with_extension("pointer-backup");
    fs::rename(&head_path, &backup).unwrap();
    fs::create_dir(&head_path).unwrap();
    let text = "🌲".repeat(PREVIEW_BYTES);
    assert!(owner
        .commit_input(&ctx, &fence.conversation_id, &fence, || {
            Ok((event("generated-input", &text), text.len() as u64))
        })
        .is_err());
    {
        let works = owner.work.lock().unwrap();
        let work = &works[&ctx.agent_id];
        assert_eq!(serde_json::to_vec(&work.head).unwrap(), before);
        assert_eq!(work.published, previous_revision);
    }
    fs::remove_dir(&head_path).unwrap();
    fs::rename(&backup, &head_path).unwrap();
    assert_eq!(fs::read(&head_path).unwrap(), previous_pointer);
    owner
        .commit_input(&ctx, &fence.conversation_id, &fence, || {
            Ok((event("generated-input", &text), text.len() as u64))
        })
        .unwrap()
        .unwrap();
    let works = owner.work.lock().unwrap();
    let work = &works[&ctx.agent_id];
    assert_eq!(work.head.row_count, 2);
    assert_eq!(work.head.committed_output_bytes, text.len() as u64);
    assert_eq!(work.signatures.len(), 2);
}

fn checkpoint_rows_retry_after_fault(stage: u8) {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(candidate(vec![event("previous", "old")]), "old".into())
        .unwrap();
    let ctx = context();
    let snap = snapshot(&ctx);
    let previous = read(&ctx, &snap, None, None, None).unwrap();
    let previous_head = head("agent-1").unwrap();
    let next = || {
        candidate(vec![
            event("previous", "old"),
            event("new-one", "one"),
            event("new-two", "two"),
        ])
    };
    owner
        .fail_checkpoint_stage
        .store(stage, std::sync::atomic::Ordering::SeqCst);
    assert!(owner.committed(next(), "new".into()).is_err());
    assert_eq!(head("agent-1").unwrap(), previous_head);
    assert!(
        read(&ctx, &snap, None, Some(&previous.revision), None)
            .unwrap()
            .unchanged
    );
    owner.committed(next(), "retry".into()).unwrap();
    let update = read(&ctx, &snap, None, Some(&previous.revision), None).unwrap();
    assert!(!update.unchanged);
    for id in ["new-one", "new-two"] {
        assert!(update.events.iter().any(|row| row.id == id), "missing {id}");
    }
    assert_eq!(update.progress, "ready");
    let all = read(&ctx, &snap, None, None, None).unwrap();
    assert_eq!(all.events.len(), 3);
}

#[test]
fn checkpoint_body_object_failure_retries_without_missing_or_duplicate_chunks() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(candidate(vec![event("previous", "old")]), "old".into())
        .unwrap();
    let previous_head = head("agent-1").unwrap();
    let text = (0..6000)
        .map(|i| format!("{i}:雪abcdef\n"))
        .collect::<String>();
    let next = || candidate(vec![event("previous", "old"), event("body", &text)]);
    owner
        .fail_checkpoint_stage
        .store(2, std::sync::atomic::Ordering::SeqCst);
    assert!(owner.committed(next(), "new".into()).is_err());
    assert_eq!(head("agent-1").unwrap(), previous_head);
    owner.committed(next(), "retry".into()).unwrap();
    for _ in 0..8 {
        if !owner.advance("agent-1").unwrap() {
            break;
        }
    }
    let ctx = context();
    let snap = snapshot(&ctx);
    let page = read(&ctx, &snap, None, None, None).unwrap();
    assert_eq!(page.progress, "ready");
    let row = page.events.iter().find(|row| row.id == "body").unwrap();
    let mut continuation = Some(row.metadata["chat_detail_ref"].as_str().unwrap().to_owned());
    let mut actual = String::new();
    let mut chunks = 0;
    while let Some(reference) = continuation {
        let chunk = read(&ctx, &snap, None, None, Some(&reference))
            .unwrap()
            .detail
            .unwrap();
        assert!(chunk.text.len() <= BODY_BYTES);
        actual.push_str(&chunk.text);
        continuation = chunk.next;
        chunks += 1;
        assert!(chunks <= 16);
    }
    assert_eq!(actual, text);
}

#[test]
fn exact_receipts_distinguish_repeated_inputs_and_fence_new_conversations() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let ctx = context();
    archive
        .append_delivered_input_with_context(ctx.clone(), "warm", None)
        .unwrap();
    let fence = input_fence(&ctx, None).unwrap().unwrap();
    let first = archive
        .append_delivered_input_receipt(ctx.clone(), "same", &fence)
        .unwrap()
        .unwrap();
    let second = archive
        .append_delivered_input_receipt(ctx.clone(), "same", &fence)
        .unwrap()
        .unwrap();
    assert_ne!(first.chat_event_id, second.chat_event_id);
    assert!(archive.has_deferred_chat_summaries("agent-1"));
    archive.flush_deferred_chat_summaries(&ctx).unwrap();
    assert!(!archive.has_deferred_chat_summaries("agent-1"));
    let wrong = InputFence {
        conversation_id: "other".into(),
        ..fence
    };
    assert!(archive
        .append_delivered_input_receipt(ctx, "same", &wrong)
        .unwrap()
        .is_none());
}

#[test]
fn immutable_roots_and_reader_byte_limits_survive_replacement_and_giant_objects() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let one = writer.put(&"one").unwrap();
    let two = writer.put(&"two").unwrap();
    let old = Some(writer.insert(&None, "key", &one).unwrap());
    let new = Some(writer.insert(&old, "key", &two).unwrap());
    let mut reader = Store::new(temp.path());
    assert_eq!(reader.get(&old, "key").unwrap(), Some(one));
    assert_eq!(reader.get(&new, "key").unwrap(), Some(two));
    let bytes = vec![b'x'; 16 * 1024 + 1];
    let reference = digest(&bytes);
    fs::write(temp.path().join(&reference), &bytes).unwrap();
    assert!(reader.read::<serde_json::Value>(&reference).is_err());
    assert!(reader.bytes < 16 * 1024);
    assert!(writer.put_bytes(&bytes).is_err());
}

#[test]
fn skipped_native_spans_and_first_disabled_eof_are_never_admitted() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source");
    fs::write(&path, b"rows\n").unwrap();
    let identity =
        crate::commands::provider_log_acquisition::native_file_identity(&File::open(path).unwrap())
            .unwrap();
    let mut policy = SourcePolicy {
        agent_id: "agent-1".into(),
        source_key: "codex:one".into(),
        identity,
        anchor: crate::commands::provider_log_acquisition::ProviderLogContinuityAnchor {
            start: 0,
            len: 5,
            sha256: digest(b"rows\n"),
        },
        generation: 1,
        unknown_before: 100,
        disabled: Vec::new(),
        open_disabled: Some(100),
        valid: true,
        framed_starts: vec![100],
    };
    assert_eq!(policy.recent_interval(100), None);
    assert!(!policy.admits(0, 100));
    policy.open_disabled = None;
    policy.disabled.push(
        crate::commands::provider_log_acquisition::ProviderLogDisabledSpan {
            start: 100,
            end: 200,
        },
    );
    assert!(!policy.admits(100, 200));
    assert!(!policy.admits(199, 210));
    assert!(policy.admits(200, 210));
    assert_eq!(policy.recent_interval(300), Some((200, 300)));
    policy.valid = false;
    assert!(!policy.admits(200, 210));
}

#[test]
fn a_native_coordinate_and_legacy_generated_ref_do_not_prove_input_correspondence() {
    let ctx = context();
    let mut record =
        super::super::narrative_from_delivered_input("2026-10-05T00:00:00Z", "identical", None, 1);
    record.event_refs = vec!["generated:conversation:1".into()];
    let mut native = event("native", "identical");
    native.metadata = json!({"provider_log": true, "log_path": "source", "chat_source_ref": "source:owned:offset:digest"});
    assert_eq!(
        super::super::records::matching_delivered_input_record_index(
            std::slice::from_ref(&record),
            &native
        )
        .unwrap(),
        None
    );
    assert!(!super::super::repair::is_bound_native_delivery(
        &record, &native
    ));
    let mut generated_record = record.clone();
    let generated = super::super::records::generated_event_from_record(
        &ctx,
        "conversation",
        &mut generated_record,
    );
    record.event_refs.push(native.id.clone());
    let mut projected = vec![generated, native];
    let before = projected.clone();
    assert!(!super::super::provenance::bind_delivered_inputs(&mut projected, &[record]).unwrap());
    assert_eq!(
        serde_json::to_value(projected).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[test]
fn native_coordinates_override_legacy_aliases_and_never_enrich_generated_inputs() {
    let mut old = event("native-one", "identical");
    old.metadata = json!({"provider_log": true, "log_path": "source", "chat_source_ref": "owned:10:digest",
        "chat_source_epoch": "epoch", "provider_turn_id": "same-turn", "legacy_event_ids": ["shared"]});
    let mut current = old.clone();
    current.id = "native-two".into();
    current.metadata["chat_source_ref"] = json!("owned:20:digest");
    assert!(!super::super::provenance::same_observation(&old, &current));
    assert_eq!(
        super::super::repair::matching_event_index(std::slice::from_ref(&old), &current).unwrap(),
        None
    );
    let mut archived = vec![old.clone()];
    assert!(!super::super::provenance::refresh_events(
        &mut archived,
        std::slice::from_ref(&current)
    )
    .unwrap());
    assert_eq!(archived, vec![old.clone()]);

    let mut legacy = old.clone();
    legacy
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("chat_source_ref");
    assert!(!super::super::provenance::same_observation(&legacy, &old));
    let mut generated = old.clone();
    generated.id = "generated:conversation:1".into();
    generated.metadata["generated"] = json!(true);
    assert!(!super::super::provenance::same_observation(
        &generated, &old
    ));

    current = old.clone();
    current.metadata["input_purpose"] = json!("continuation_input");
    assert!(super::super::provenance::same_observation(&old, &current));
    let mut archived = vec![old];
    assert!(super::super::provenance::refresh_events(&mut archived, &[current]).unwrap());
    assert_eq!(archived[0].metadata["input_purpose"], "continuation_input");
}

fn owned_native_source(
    path: &Path,
    contents: &str,
) -> (
    crate::commands::chat::AgentArchiveCaptureSnapshot,
    SourcePolicy,
) {
    fs::write(path, contents).unwrap();
    let mut snapshot = snapshot(&context());
    snapshot.provider = "codex".into();
    snapshot.resume_session = Some("one".into());
    snapshot.log_path = Some(path.to_owned());
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let anchor = &contents.as_bytes()[..contents.len().min(4096)];
    let policy = SourcePolicy {
        agent_id: ctx.agent_id.clone(),
        source_key: ctx.provider_source_key.clone().unwrap(),
        identity: crate::commands::provider_log_acquisition::native_file_identity(
            &File::open(path).unwrap(),
        )
        .unwrap(),
        anchor: crate::commands::provider_log_acquisition::ProviderLogContinuityAnchor {
            start: 0,
            len: anchor.len() as u64,
            sha256: digest(anchor),
        },
        generation: 1,
        unknown_before: 0,
        disabled: Vec::new(),
        open_disabled: None,
        valid: true,
        framed_starts: vec![0],
    };
    write_json_atomic(&policy_path(&ctx.agent_id).unwrap(), &Some(&policy)).unwrap();
    (snapshot, policy)
}

fn native_message(text: &str) -> String {
    format!(
        "{}\n",
        json!({"type": "event_msg", "payload": {"type": "user_message", "message": text}})
    )
}

#[test]
fn provisional_qualified_pair_survives_restart_with_both_physical_aliases() {
    let (_guard, temp) = isolated_home();
    let contents = format!(
        "{}\n{}",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl")
    );
    let (snapshot, _) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let physical = crate::commands::chat_recent_seed::read(&snapshot, None)
        .unwrap()
        .events;
    assert_eq!(physical.len(), 2);
    assert_ne!(physical[0].id, physical[1].id);
    advance_source_index(&snapshot).unwrap();
    let first = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.aliases.len(), 2);
    assert!(physical.iter().all(|event| first
        .aliases
        .iter()
        .any(|alias| alias.observation_id == event.id)));
    // Restore only immutable pointers. No archive hydration or native replay.
    let restarted = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(restarted.events[0].id, first.events[0].id);
    assert_eq!(
        restarted
            .aliases
            .iter()
            .map(|alias| (&alias.observation_id, &alias.canonical_id))
            .collect::<Vec<_>>(),
        first
            .aliases
            .iter()
            .map(|alias| (&alias.observation_id, &alias.canonical_id))
            .collect::<Vec<_>>()
    );
    assert!(restarted.records_decoded < 512);
    assert!(restarted.bytes_read < 2 * 1024 * 1024);
    assert!(head("agent-1").unwrap().is_none());
}

#[test]
fn normal_canonical_capture_groups_qualified_pair_without_rewriting_archive() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("source.jsonl");
    let contents = format!(
        "{}\n{}",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl")
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
    let physical = batch.events.clone();
    assert_eq!(physical.len(), 2);
    let archive = ConversationArchiveState::default();
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &batch.events, None, &batch.next)
        .unwrap();
    for _ in 0..16 {
        if !archive.chat_projection.advance("agent-1").unwrap() {
            break;
        }
    }
    let page = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.aliases.len(), 2);
    let saved = archive.chat_events_for_agent("agent-1").unwrap();
    assert!(physical
        .iter()
        .all(|event| saved.iter().any(|row| row.id == event.id
            && row.metadata["chat_source_ref"] == event.metadata["chat_source_ref"])));
    drop(archive);
    let restarted = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(restarted.events[0].id, page.events[0].id);
    assert!(restarted.bytes_read < 2 * 1024 * 1024);
}

#[test]
fn provisional_explicit_turn_pair_joins_across_windows_and_older_page_boundary() {
    let (_guard, temp) = isolated_home();
    let mut contents = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}})
    );
    let fixture: Vec<_> = include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl")
        .lines()
        .collect();
    contents.push_str(fixture[0]);
    contents.push('\n');
    contents.push_str(fixture[1]);
    contents.push('\n');
    for index in 0..90 {
        contents.push_str(&native_message(&format!("distinct-{index}")));
    }
    let mut mirror: serde_json::Value = serde_json::from_str(fixture[2]).unwrap();
    mirror["turn_id"] = json!("provider-turn-a");
    contents.push_str(&format!("{mirror}\n"));
    let (snapshot, _) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    assert!(advance_source_index(&snapshot).unwrap());
    let initial = read(&ctx, &snapshot, None, None, None).unwrap();
    let cursor = initial.next_before.unwrap();
    for _ in 0..8 {
        if !advance_source_index(&snapshot).unwrap() {
            break;
        }
    }
    let recent = read(&ctx, &snapshot, None, None, None).unwrap();
    let grouped = recent
        .events
        .iter()
        .find(|event| event.text.as_deref() == Some("Inspect the archive."))
        .unwrap();
    assert!(grouped.id.starts_with("display:"));
    assert_eq!(
        grouped.metadata["chat_display_member_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let older = read(&ctx, &snapshot, Some(&cursor), None, None).unwrap();
    assert!(older.events.iter().any(|event| event.id == grouped.id));
    assert!(older.records_decoded < 512);
    assert!(older.bytes_read < 2 * 1024 * 1024);
    assert!(head("agent-1").unwrap().is_none());
}

fn finish_projection(archive: &ConversationArchiveState) {
    for _ in 0..128 {
        if !archive.chat_projection.advance("agent-1").unwrap() {
            return;
        }
    }
    panic!("bounded projection checkpoints did not complete");
}

#[test]
fn codex_original_sequence_bridge_and_user_mirror_share_normal_paged_relation() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("codex-legacy.jsonl");
    let contents = format!(
        "{}\n{}",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl")
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let mut legacy = crate::providers::chat_transcript::normalize_chat_lines(
        "agent-1",
        "codex",
        contents.lines(),
    )
    .into_iter()
    .find(|event| event.source.as_deref() == Some("response_item"))
    .unwrap();
    legacy.id =
        crate::commands::chat::archive_identity::stable_provider_log_event_id(&legacy, &path);
    legacy.metadata["provider_log"] = json!(true);
    legacy.metadata["log_path"] = json!(path.to_string_lossy());
    legacy.metadata["chat_legacy_source_sequence"] = json!(legacy.sequence.unwrap());
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(ctx.clone(), std::slice::from_ref(&legacy))
        .unwrap();
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &batch.events, None, &batch.next)
        .unwrap();
    finish_projection(&archive);
    let page = read(&ctx, &snapshot, None, None, None).unwrap();
    let rows: Vec<_> = page
        .events
        .iter()
        .filter(|event| event.role == Some(AgentChatRole::User))
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].metadata["chat_display_member_ids"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for id in std::iter::once(&legacy.id).chain(
        batch
            .events
            .iter()
            .filter(|event| event.kind == AgentChatEventKind::Message)
            .map(|event| &event.id),
    ) {
        assert!(page
            .aliases
            .iter()
            .any(|alias| &alias.observation_id == id && alias.canonical_id == rows[0].id));
    }
    assert_eq!(
        archive
            .chat_events_for_agent("agent-1")
            .unwrap()
            .iter()
            .filter(|event| event.kind == AgentChatEventKind::Message)
            .count(),
        3
    );
}

#[test]
fn unframed_provisional_suffix_cannot_qualify_until_completed_bounded_recount() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("partial.jsonl");
    let contents = format!(
        "{}\n{}{}",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl"),
        r#"{"type":"ignored_control""#
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    for _ in 0..8 {
        if !advance_source_index(&snapshot).unwrap() {
            break;
        }
    }
    let incomplete = read(&ctx, &snapshot, None, None, None).unwrap();
    assert!(incomplete
        .events
        .iter()
        .all(|event| !event.id.starts_with("display:")));
    append_native_line(&path, "}");
    for _ in 0..16 {
        if !advance_source_index(&snapshot).unwrap() {
            break;
        }
    }
    let complete = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(complete.events.len(), 1);
    assert!(complete.events[0].id.starts_with("display:"));
    assert_eq!(complete.generation, incomplete.generation);
}

#[test]
fn repeated_same_turn_after_qualified_pair_retracts_aliases_and_keeps_physical_rows_usable() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("repeat.jsonl");
    let contents = format!(
        "{}\n{}",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        include_str!("../../providers/fixtures/codex-user-input-mirror.jsonl")
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &first.events, None, &first.next)
        .unwrap();
    finish_projection(&archive);
    let qualified = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(qualified.events.len(), 1);
    let old_display_id = qualified.events[0].id.clone();
    assert!(old_display_id.starts_with("display:"));
    append_native_line(&path, &json!({"type":"event_msg","turn_id":"provider-turn-a","payload":{"type":"user_message","client_id":"repeated-client","message":"Inspect the archive."}}).to_string());
    // Unconsumed bytes revoke the old completion certificate before capture.
    let pending = read(&ctx, &snapshot, None, None, None).unwrap();
    assert!(pending.reset);
    assert!(pending
        .events
        .iter()
        .all(|event| !event.id.starts_with("display:")));
    let mut next = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        Some(first.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut next.events, "codex", &path);
    archive
        .append_provider_log_batch_with_context(
            ctx.clone(),
            &next.events,
            Some(&first.next),
            &next.next,
        )
        .unwrap();
    finish_projection(&archive);
    let retracted = read(&ctx, &snapshot, None, Some(&qualified.revision), None).unwrap();
    assert_eq!(
        retracted
            .events
            .iter()
            .filter(|event| event.kind == AgentChatEventKind::Message)
            .count(),
        3
    );
    assert!(retracted
        .events
        .iter()
        .all(|event| !event.id.starts_with("display:")));
    assert!(retracted.removed_ids.contains(&old_display_id));
    assert!(!retracted
        .aliases
        .iter()
        .any(|alias| alias.canonical_id == old_display_id));
    assert_eq!(
        archive
            .chat_events_for_agent("agent-1")
            .unwrap()
            .iter()
            .filter(|event| event.kind == AgentChatEventKind::Message)
            .count(),
        3
    );
}

fn append_native_line(path: &Path, line: &str) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{line}").unwrap();
    file.sync_all().unwrap();
}

fn pi_legacy_from_original_algorithm(contents: &str, path: &Path) -> AgentChatEvent {
    let mut legacy =
        crate::providers::chat_transcript::normalize_chat_lines("agent-1", "pi", contents.lines())
            .into_iter()
            .find(|event| event.kind == AgentChatEventKind::Message)
            .unwrap();
    // The published original projection used the nested message ID. This
    // fixture has none; today's adapter correctly retains the envelope ID.
    legacy.turn_id = None;
    legacy.id =
        crate::commands::chat::archive_identity::stable_provider_log_event_id(&legacy, path);
    legacy.metadata["provider_log"] = json!(true);
    legacy.metadata["log_path"] = json!(path.to_string_lossy());
    legacy.metadata["chat_legacy_source_sequence"] = json!(legacy.sequence.unwrap());
    legacy
}

#[test]
fn pi_original_algorithm_sequence_bridge_reaches_normal_recent_older_and_restart_pages() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("pi.jsonl");
    let mut contents = format!(
        "{}\n{}\n",
        json!({"type":"session","id":"one"}),
        json!({"type":"message","id":"native-entry","message":{"role":"user","content":[{"type":"text","text":"legacy prompt"}]}})
    );
    let legacy = pi_legacy_from_original_algorithm(&contents, &path);
    for index in 0..90 {
        contents.push_str(&format!("{}\n", json!({"type":"message","id":format!("entry-{index}"),"message":{"role":"user","content":[{"type":"text","text":format!("distinct-{index}")}]}})));
    }
    fs::write(&path, &contents).unwrap();
    let mut snapshot = snapshot(&context());
    snapshot.provider = "pi".into();
    snapshot.resume_session = Some("one".into());
    snapshot.log_path = Some(path.clone());
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(ctx.clone(), std::slice::from_ref(&legacy))
        .unwrap();
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "pi",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "pi", &path);
    let native = batch
        .events
        .iter()
        .find(|event| event.text.as_deref() == Some("legacy prompt"))
        .unwrap()
        .clone();
    assert_eq!(native.turn_id.as_deref(), Some("native-entry"));
    assert_eq!(
        native.metadata["chat_compatibility_legacy_id"],
        json!(legacy.id)
    );
    assert_eq!(
        native.metadata["chat_compatibility_source_sequence"],
        legacy.metadata["chat_legacy_source_sequence"]
    );
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &batch.events, None, &batch.next)
        .unwrap();
    finish_projection(&archive);
    let recent = read(&ctx, &snapshot, None, None, None).unwrap();
    let older = read(&ctx, &snapshot, recent.next_before.as_deref(), None, None).unwrap();
    let grouped = older
        .events
        .iter()
        .find(|event| event.text.as_deref() == Some("legacy prompt"))
        .unwrap();
    assert_eq!(grouped.id, legacy.id);
    assert_eq!(
        older
            .events
            .iter()
            .filter(|event| event.text.as_deref() == Some("legacy prompt"))
            .count(),
        1
    );
    assert!(older
        .aliases
        .iter()
        .any(|alias| alias.observation_id == native.id && alias.canonical_id == legacy.id));
    assert!(older.records_decoded < 512 && older.bytes_read < 2 * 1024 * 1024);
    let persisted = archive.chat_events_for_agent("agent-1").unwrap();
    assert!(persisted
        .iter()
        .any(|event| event.id == legacy.id && !event.metadata["chat_source_ref"].is_string()));
    assert!(persisted.iter().any(
        |event| event.id == native.id && event.metadata["chat_source_ref"] == json!(native.id)
    ));
    drop(archive);
    let restarted = read(&ctx, &snapshot, recent.next_before.as_deref(), None, None).unwrap();
    assert_eq!(
        restarted
            .events
            .iter()
            .find(|event| event.text.as_deref() == Some("legacy prompt"))
            .unwrap()
            .id,
        legacy.id
    );
}

#[test]
fn pi_repeated_original_legacy_envelopes_refuse_sequence_bridge_without_transcript_failure() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("pi.jsonl");
    let contents = format!(
        "{}\n{}\n",
        json!({"type":"session","id":"one"}),
        json!({"type":"message","id":"native-entry","message":{"role":"user","content":[{"type":"text","text":"same"}]}})
    );
    fs::write(&path, &contents).unwrap();
    let legacy = pi_legacy_from_original_algorithm(&contents, &path);
    let mut snapshot = snapshot(&context());
    snapshot.provider = "pi".into();
    snapshot.resume_session = Some("one".into());
    snapshot.log_path = Some(path.clone());
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(ctx.clone(), std::slice::from_ref(&legacy))
        .unwrap();
    let conversation = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let events_path = super::super::conversation_dir("agent-1", &conversation)
        .unwrap()
        .join("events.jsonl");
    // A persisted pre-migration archive can contain two original envelopes
    // with the same old algorithm ID. Count both instead of selecting one ID.
    let mut historical: Vec<AgentChatEvent> =
        super::super::read_jsonl_records(&events_path).unwrap();
    historical.push(legacy.clone());
    wardian_core::conversations::write_jsonl_atomic(&events_path, &historical).unwrap();
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "pi",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "pi", &path);
    let native_id = batch
        .events
        .iter()
        .find(|event| event.kind == AgentChatEventKind::Message)
        .unwrap()
        .id
        .clone();
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &batch.events, None, &batch.next)
        .unwrap();
    finish_projection(&archive);
    let page = read(&ctx, &snapshot, None, None, None).unwrap();
    assert!(page.events.iter().any(|event| event.id == legacy.id));
    assert!(page.events.iter().any(|event| event.id == native_id));
    assert!(!page
        .aliases
        .iter()
        .any(|alias| alias.observation_id == native_id && alias.canonical_id == legacy.id));
    assert!(
        archive
            .chat_events_for_agent("agent-1")
            .unwrap()
            .iter()
            .filter(|event| event.id == legacy.id)
            .count()
            >= 2
    );
}

#[test]
fn canonical_assistant_stream_final_joins_across_acquisition_batches_and_restart() {
    let (_guard, temp) = isolated_home();
    for (index, source) in ["response_item", "item.completed"].iter().enumerate() {
        let agent = format!("assistant-agent-{index}");
        let path = temp.path().join(format!("assistant-{index}.jsonl"));
        let text = "answer".repeat(1800);
        let contents = format!(
            "{}\n{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"one"}}),
            json!({"type":"turn_context","payload":{"turn_id":"turn-a"}}),
            json!({"type":"event_msg","payload":{"type":"agent_message","message":text}})
        );
        fs::write(&path, &contents).unwrap();
        let mut snapshot = snapshot(&context());
        snapshot.session_id = agent.clone();
        snapshot.provider = "codex".into();
        snapshot.resume_session = Some("one".into());
        snapshot.log_path = Some(path.clone());
        let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
        let archive = ConversationArchiveState::default();
        let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            &agent,
            "codex",
            &path,
            ctx.provider_source_key.as_deref().unwrap(),
            None,
            true,
        )
        .unwrap();
        crate::commands::chat::decorate_forward_provider_log_events(
            &mut first.events,
            "codex",
            &path,
        );
        let live = first
            .events
            .iter()
            .find(|event| event.role == Some(AgentChatRole::Assistant))
            .unwrap()
            .clone();
        assert!(live.turn_id.is_none());
        archive
            .append_provider_log_batch_with_context(ctx.clone(), &first.events, None, &first.next)
            .unwrap();
        let before = read(&ctx, &snapshot, None, None, None).unwrap();
        assert!(before.events.iter().any(|event| event.id == live.id));
        let mut completion = json!({"type":source});
        completion[if *source == "item.completed" {
            "item"
        } else {
            "payload"
        }] = json!({"type":"message","role":"assistant","id":"final-item","phase":"final_answer","content":[{"type":"output_text","text":text}],"internal_chat_message_metadata_passthrough":{"turn_id":"turn-a"}});
        append_native_line(&path, &completion.to_string());
        let mut second = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            &agent,
            "codex",
            &path,
            ctx.provider_source_key.as_deref().unwrap(),
            Some(first.next.clone()),
            true,
        )
        .unwrap();
        crate::commands::chat::decorate_forward_provider_log_events(
            &mut second.events,
            "codex",
            &path,
        );
        let final_event = second
            .events
            .iter()
            .find(|event| event.role == Some(AgentChatRole::Assistant))
            .unwrap()
            .clone();
        assert_eq!(final_event.turn_id.as_deref(), Some("final-item"));
        assert_ne!(live.id, final_event.id);
        archive
            .append_provider_log_batch_with_context(
                ctx.clone(),
                &second.events,
                Some(&first.next),
                &second.next,
            )
            .unwrap();
        let mut pending = true;
        for _ in 0..32 {
            pending = archive.chat_projection.advance(&agent).unwrap();
            if !pending {
                break;
            }
        }
        assert!(!pending);
        let page = read(&ctx, &snapshot, None, None, None).unwrap();
        let assistants: Vec<_> = page
            .events
            .iter()
            .filter(|event| event.role == Some(AgentChatRole::Assistant))
            .collect();
        assert_eq!(assistants.len(), 1);
        let display_id = assistants[0].id.clone();
        assert!(display_id.starts_with("display:"));
        assert_eq!(
            assistants[0].metadata["codex_display_text_bytes"].as_u64(),
            Some(text.len() as u64)
        );
        assert!(assistants[0].text.as_ref().unwrap().len() < text.len());
        for physical in [&live.id, &final_event.id] {
            assert!(
                page.aliases
                    .iter()
                    .any(|alias| &alias.observation_id == physical
                        && alias.canonical_id == display_id)
            );
        }
        let physical = archive.chat_events_for_agent(&agent).unwrap();
        assert!(physical.iter().any(|event| event.id == live.id));
        assert!(physical.iter().any(|event| event.id == final_event.id));
        drop(archive);
        let restarted = read(&ctx, &snapshot, None, None, None).unwrap();
        assert_eq!(
            restarted
                .events
                .iter()
                .find(|event| event.role == Some(AgentChatRole::Assistant))
                .unwrap()
                .id,
            display_id
        );
    }
}

#[test]
fn disabled_native_interval_cannot_complete_a_pair_or_legacy_claim() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("disabled.jsonl");
    let contents = format!(
        "{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn-a"}}),
        json!({"type":"event_msg","payload":{"type":"agent_message","message":"same"}})
    );
    fs::write(&path, contents).unwrap();
    let mut snapshot = snapshot(&context());
    snapshot.provider = "codex".into();
    snapshot.resume_session = Some("one".into());
    snapshot.log_path = Some(path.clone());
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &first.events, None, &first.next)
        .unwrap();
    let disabled = crate::commands::provider_log_acquisition::observe_provider_log_policy(
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        Some(first.next.clone()),
        false,
        true,
    )
    .unwrap();
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &[], Some(&first.next), &disabled.next)
        .unwrap();
    append_native_line(&path, &json!({"type":"response_item","payload":{"type":"message","role":"assistant","id":"private-final","phase":"final_answer","text":"same","internal_chat_message_metadata_passthrough":{"turn_id":"turn-a"}}}).to_string());
    let enabled = crate::commands::provider_log_acquisition::observe_provider_log_policy(
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        Some(disabled.next.clone()),
        true,
        true,
    )
    .unwrap();
    archive
        .append_provider_log_batch_with_context(
            ctx.clone(),
            &[],
            Some(&disabled.next),
            &enabled.next,
        )
        .unwrap();
    append_native_line(
        &path,
        native_message("usable after disabled span").trim_end(),
    );
    let skipped = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        Some(enabled.next.clone()),
        true,
    )
    .unwrap();
    assert!(skipped.events.is_empty());
    archive
        .append_provider_log_batch_with_context(
            ctx.clone(),
            &[],
            Some(&enabled.next),
            &skipped.next,
        )
        .unwrap();
    let mut visible = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        Some(skipped.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut visible.events,
        "codex",
        &path,
    );
    archive
        .append_provider_log_batch_with_context(
            ctx.clone(),
            &visible.events,
            Some(&skipped.next),
            &visible.next,
        )
        .unwrap();
    finish_projection(&archive);
    for _ in 0..16 {
        if !advance_source_index(&snapshot).unwrap() {
            break;
        }
    }
    let page = read(&ctx, &snapshot, None, None, None).unwrap();
    assert!(page
        .events
        .iter()
        .any(|event| event.text.as_deref() == Some("usable after disabled span")));
    assert!(!page
        .events
        .iter()
        .any(|event| event.turn_id.as_deref() == Some("private-final")));
    assert!(!page
        .events
        .iter()
        .any(|event| event.id.starts_with("display:")));
    assert!(!page
        .aliases
        .iter()
        .any(|alias| alias.canonical_id.starts_with("display:")));
    assert!(
        !super::super::chat_logical_index::source_proof(&ctx)
            .unwrap()
            .unwrap()
            .sequence_trusted
    );
}

#[test]
fn provisional_earlier_window_pairs_activate_from_persisted_completion_queue() {
    let (_guard, temp) = isolated_home();
    let mut contents = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}})
    );
    for index in 0..120 {
        contents.push_str(&native_message(&format!("older-{index}")));
    }
    for index in 0..12 {
        let turn = format!("turn-{index}");
        let text = format!("paired-{index}");
        contents.push_str(&format!("{}\n{}\n{}\n",
            json!({"type":"turn_context","payload":{"turn_id":turn}}),
            json!({"type":"response_item","payload":{"type":"message","id":format!("request-{index}"),"role":"user","content":[{"type":"input_text","text":text}],"internal_chat_message_metadata_passthrough":{"turn_id":turn,"content_item_kinds":["user.text"]}}}),
            json!({"type":"event_msg","turn_id":turn,"payload":{"type":"user_message","client_id":format!("client-{index}"),"message":text}})));
    }
    let (snapshot, _) = owned_native_source(&temp.path().join("activation.jsonl"), &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    assert!(advance_source_index(&snapshot).unwrap());
    let early = read(&ctx, &snapshot, None, None, None).unwrap();
    assert!(early.events.iter().any(|event| event
        .text
        .as_deref()
        .is_some_and(|text| text.starts_with("paired-"))));
    assert!(early
        .events
        .iter()
        .all(|event| !event.id.starts_with("display:")));
    assert_eq!(early.progress, "indexing");
    // Each checkpoint restores its owner from disk. The pair-containing tail
    // is not revisited while older windows complete the occurrence counts.
    let mut pending = true;
    for _ in 0..32 {
        pending = advance_source_index(&snapshot).unwrap();
        if !pending {
            break;
        }
    }
    assert!(!pending);
    let complete = read(&ctx, &snapshot, None, None, None).unwrap();
    let paired: Vec<_> = complete
        .events
        .iter()
        .filter(|event| {
            event
                .text
                .as_deref()
                .is_some_and(|text| text.starts_with("paired-"))
        })
        .collect();
    assert_eq!(paired.len(), 12);
    assert!(paired.iter().all(|event| event.id.starts_with("display:")
        && event.metadata["chat_display_member_ids"]
            .as_array()
            .unwrap()
            .len()
            == 2));
    assert_eq!(complete.generation, early.generation);
    assert!(complete.records_decoded < 512 && complete.bytes_read < 2 * 1024 * 1024);
    let restarted = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(
        restarted
            .events
            .iter()
            .map(|event| &event.id)
            .collect::<Vec<_>>(),
        complete
            .events
            .iter()
            .map(|event| &event.id)
            .collect::<Vec<_>>()
    );
}

#[test]
fn pi_repeated_native_old_algorithm_ids_do_not_bridge_an_unsequenced_legacy_row() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("pi-repeated.jsonl");
    let mut contents = format!("{}\n", json!({"type":"session","id":"one"}));
    for id in ["entry-a", "entry-b"] {
        contents.push_str(&format!("{}\n", json!({"type":"message","id":id,"message":{"role":"user","content":[{"type":"text","text":"same"}]}})));
    }
    fs::write(&path, &contents).unwrap();
    let mut legacy = pi_legacy_from_original_algorithm(&contents, &path);
    legacy
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("chat_legacy_source_sequence");
    let mut snapshot = snapshot(&context());
    snapshot.provider = "pi".into();
    snapshot.resume_session = Some("one".into());
    snapshot.log_path = Some(path.clone());
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    archive
        .append_chat_events_with_context(ctx.clone(), &[legacy.clone()])
        .unwrap();
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "pi",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "pi", &path);
    let native: Vec<_> = batch
        .events
        .iter()
        .filter(|event| event.kind == AgentChatEventKind::Message)
        .collect();
    assert_eq!(native.len(), 2);
    assert_ne!(native[0].id, native[1].id);
    assert!(native
        .iter()
        .all(|event| event.metadata["chat_compatibility_legacy_id"] == json!(legacy.id)));
    let native_ids: Vec<_> = native.iter().map(|event| event.id.clone()).collect();
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &batch.events, None, &batch.next)
        .unwrap();
    finish_projection(&archive);
    let page = read(&ctx, &snapshot, None, None, None).unwrap();
    assert_eq!(
        page.events
            .iter()
            .filter(|event| event.kind == AgentChatEventKind::Message)
            .count(),
        3
    );
    assert!(native_ids
        .iter()
        .all(|id| page.events.iter().any(|event| &event.id == id)));
    assert!(
        !page
            .aliases
            .iter()
            .any(|alias| native_ids.contains(&alias.observation_id)
                && alias.canonical_id == legacy.id)
    );
}

#[test]
fn provisional_rows_are_owned_distinct_and_available_without_a_canonical_head() {
    let (_guard, temp) = isolated_home();
    let contents = format!(
        "{}\n{}{}",
        json!({"type": "session_meta", "payload": {"id": "one"}}),
        native_message("identical"),
        native_message("identical")
    );
    let (snapshot, _) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    assert_eq!(seed.events.len(), 2);
    assert_ne!(seed.events[0].id, seed.events[1].id);
    assert!(seed
        .events
        .iter()
        .all(|event| event.metadata["chat_provisional"] == true));
    assert!(head("agent-1").unwrap().is_none());
    assert!(seed.bytes_read < 2 * 1024 * 1024);
    assert!(seed.records_decoded <= 84);
    let unchanged =
        crate::commands::chat_recent_seed::read(&snapshot, Some(&seed.revision)).unwrap();
    assert!(unchanged.unchanged);
    assert!(unchanged.events.is_empty());
    let mut foreign = snapshot.clone();
    foreign.resume_session = Some("foreign".into());
    assert!(crate::commands::chat_recent_seed::read(&foreign, None)
        .unwrap()
        .events
        .is_empty());
}

#[test]
fn provisional_policy_rejects_skipped_details_rewrites_and_rotation() {
    let (_guard, temp) = isolated_home();
    let prefix = format!(
        "{}\n",
        json!({"type": "session_meta", "payload": {"id": "one"}})
    );
    let secret = native_message(&"private".repeat(300));
    let contents = format!("{prefix}{secret}{}", native_message("visible"));
    let path = temp.path().join("source.jsonl");
    let (snapshot, mut policy) = owned_native_source(&path, &contents);
    let seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let detail_ref = seed.events[0].metadata["chat_detail_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        crate::commands::chat_recent_seed::detail(&snapshot, &detail_ref)
            .unwrap()
            .text
            .contains("private")
    );
    policy.unknown_before = (prefix.len() + secret.len()) as u64;
    policy.framed_starts.push(policy.unknown_before);
    policy.generation += 1;
    write_json_atomic(&policy_path("agent-1").unwrap(), &Some(&policy)).unwrap();
    let visible = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    assert_eq!(visible.events.len(), 1);
    assert_eq!(visible.events[0].text.as_deref(), Some("visible"));
    assert!(crate::commands::chat_recent_seed::detail(&snapshot, &detail_ref).is_err());
    assert!(crate::commands::chat_recent_seed::detail(&snapshot, "seed:arbitrary:path").is_err());

    fs::write(&path, contents.replace("visible", "changed")).unwrap();
    assert!(crate::commands::chat_recent_seed::read(&snapshot, None)
        .unwrap()
        .events
        .is_empty());
    fs::rename(&path, temp.path().join("retired.jsonl")).unwrap();
    fs::write(&path, &contents).unwrap();
    assert!(crate::commands::chat_recent_seed::read(&snapshot, None)
        .unwrap()
        .events
        .is_empty());
}

#[test]
fn a_giant_unindexed_final_record_reports_progress_with_bounded_reads() {
    let (_guard, temp) = isolated_home();
    let contents = format!(
        "{}\n{}",
        json!({"type": "session_meta", "payload": {"id": "one"}}),
        native_message(&"x".repeat(2 * 1024 * 1024))
    );
    let (snapshot, _) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    assert_eq!(seed.progress, "oversized_record");
    assert!(seed.events.is_empty());
    assert!(seed.bytes_read < 400 * 1024);
    assert!(seed.records_decoded <= 84);
}

#[test]
fn ipc_trimming_keeps_recent_rows_and_continues_before_the_first_retained_key() {
    let (_guard, _temp) = isolated_home();
    let owner = ProjectionOwner::default();
    owner
        .committed(candidate(vec![event("one", "one")]), "seal".into())
        .unwrap();
    let mut page = read(&context(), &snapshot(&context()), None, None, None).unwrap();
    page.events = (0..80)
        .map(|index| {
            let mut row = event(&format!("row-{index}"), &"x".repeat(8000));
            row.metadata["chat_page_key"] = json!(format!("{index:020}"));
            row
        })
        .collect();
    let page = finish(page).unwrap();
    assert!(!page.events.is_empty());
    assert!(page.events.len() < 80);
    assert_eq!(page.events.last().unwrap().id, "row-79");
    assert!(serde_json::to_vec(&page).unwrap().len() <= IPC_BYTES);
    let (_, before) = page.next_before.as_ref().unwrap().split_once(':').unwrap();
    assert_eq!(
        before,
        page.events[0].metadata["chat_page_key"].as_str().unwrap()
    );
}

#[test]
fn recent_and_forward_adapters_share_the_same_physical_observation_identity() {
    let (_guard, temp) = isolated_home();
    let contents = format!(
        "{}\n{}{}",
        json!({"type": "session_meta", "payload": {"id": "one"}}),
        native_message("same"),
        native_message("same")
    );
    let path = temp.path().join("source.jsonl");
    let (snapshot, _) = owned_native_source(&path, &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        ctx.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    publish_policy(&ctx, &batch.next).unwrap();
    let seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let forward: Vec<_> = batch
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("same"))
        .map(|event| event.metadata["chat_source_ref"].as_str().unwrap())
        .collect();
    assert_eq!(forward.len(), 2);
    assert_eq!(
        seed.events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        forward
    );
}

#[test]
fn legacy_generated_links_are_downgraded_only_in_the_display_projection() {
    let (_guard, _temp) = isolated_home();
    let archive = ConversationArchiveState::default();
    let ctx = context();
    archive
        .append_delivered_input_with_context(ctx.clone(), "same", None)
        .unwrap();
    let fence = input_fence(&ctx, None).unwrap().unwrap();
    let directory = super::super::conversation_dir("agent-1", &fence.conversation_id).unwrap();
    let path = directory.join("events.jsonl");
    let mut events: Vec<AgentChatEvent> =
        wardian_core::conversations::read_jsonl_records(&path).unwrap();
    let generated = events
        .iter_mut()
        .find(|event| event.metadata["generated"] == true)
        .unwrap();
    let generated_id = generated.id.clone();
    generated.metadata["chat_source_ref"] = json!("legacy:text-selected-native");
    generated.metadata["legacy_event_ids"] = json!(["legacy-native"]);
    wardian_core::conversations::write_jsonl_atomic(&path, &events).unwrap();
    let before = fs::read(&path).unwrap();
    archive
        .publish_chat_candidate(Candidate {
            context: ctx.clone(),
            conversation_id: fence.conversation_id,
            events,
            committed_output_bytes: before.len() as u64,
            verified_ids: Default::default(),
            generated_ids: [generated_id.clone()].into_iter().collect(),
            generated_input_bindings: Default::default(),
            source_epoch: None,
        })
        .unwrap();
    let page = read(&ctx, &snapshot(&ctx), None, None, None).unwrap();
    let row = page
        .events
        .iter()
        .find(|row| row.id == generated_id)
        .unwrap();
    assert!(!row.metadata["chat_source_ref"].is_string());
    assert!(!row.metadata["legacy_event_ids"].is_array());
    assert!(page.aliases.is_empty());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn cold_eof_indexes_recent_first_and_resumes_older_without_a_canonical_candidate() {
    let (_guard, temp) = isolated_home();
    let mut contents = format!(
        "{}\n",
        json!({"type": "session_meta", "payload": {"id": "one"}})
    );
    for index in 0..150 {
        contents.push_str(&native_message(&format!("message-{index}")));
    }
    let (snapshot, _) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    assert!(advance_source_index(&snapshot).unwrap());
    let first = read(&ctx, &snapshot, None, None, None).unwrap();
    assert!(first.conversation_id.is_none());
    assert!(first.generation.is_some());
    assert_eq!(first.events.len(), 80);
    assert_eq!(
        first.events.last().unwrap().text.as_deref(),
        Some("message-149")
    );
    let cursor = first.next_before.clone().unwrap();
    assert!(cursor.starts_with("source:"));
    let unchanged = read(&ctx, &snapshot, None, Some(&first.revision), None).unwrap();
    assert!(unchanged.unchanged);
    assert!(unchanged.events.is_empty());
    assert!(!unchanged.reset);
    assert_eq!(unchanged.conversation_id, None);
    assert_eq!(unchanged.generation, first.generation);
    assert_eq!(unchanged.source_epoch, first.source_epoch);
    assert_eq!(unchanged.progress, first.progress);
    assert!(unchanged.records_decoded < 16);
    assert!(unchanged.bytes_read < 2 * 1024 * 1024);
    // Each call restores the immutable checkpoint from disk; no in-memory
    // canonical handle or full archive replay is necessary after restart.
    let mut pending = true;
    for _ in 0..5 {
        pending = advance_source_index(&snapshot).unwrap();
        if !pending {
            break;
        }
    }
    assert!(!pending);
    let older = read(&ctx, &snapshot, Some(&cursor), None, None).unwrap();
    assert_eq!(older.events.len(), 70);
    assert_eq!(
        older.events.first().unwrap().text.as_deref(),
        Some("message-0")
    );
    assert!(older.next_before.is_none());
    assert_eq!(older.generation, first.generation);
    assert!(older.bytes_read < 2 * 1024 * 1024);
    assert!(older.records_decoded < 512);
    assert!(head("agent-1").unwrap().is_none());
}

#[test]
fn provisional_generation_cursors_and_details_expire_on_private_policy_change() {
    let (_guard, temp) = isolated_home();
    let mut contents = format!(
        "{}\n",
        json!({"type": "session_meta", "payload": {"id": "one"}})
    );
    for _ in 0..90 {
        contents.push_str(&native_message(&"detail".repeat(250)));
    }
    let (snapshot, mut policy) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    advance_source_index(&snapshot).unwrap();
    let first = read(&ctx, &snapshot, None, None, None).unwrap();
    let cursor = first.next_before.as_deref().unwrap();
    let reference = first.events.last().unwrap().metadata["chat_detail_ref"]
        .as_str()
        .unwrap();
    let detail = read(&ctx, &snapshot, None, None, Some(reference)).unwrap();
    assert_eq!(detail.generation, first.generation);
    assert!(detail.detail.unwrap().text.len() > PREVIEW_BYTES);
    policy.generation += 1;
    write_json_atomic(&policy_path("agent-1").unwrap(), &Some(&policy)).unwrap();
    assert!(read(&ctx, &snapshot, None, None, Some(reference)).is_err());
    advance_source_index(&snapshot).unwrap();
    let reset = read(&ctx, &snapshot, Some(cursor), None, None).unwrap();
    assert!(reset.reset);
    assert_ne!(reset.generation, first.generation);
    assert!(reset.events.len() <= 80);
}

#[test]
fn bounded_background_checkpoints_can_reach_a_prompt_before_a_giant_record() {
    let (_guard, temp) = isolated_home();
    let contents = format!(
        "{}\n{}{}",
        json!({"type": "session_meta", "payload": {"id": "one"}}),
        native_message("usable older prompt"),
        native_message(&"x".repeat(2 * 1024 * 1024))
    );
    let (snapshot, _) = owned_native_source(&temp.path().join("source.jsonl"), &contents);
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    for _ in 0..32 {
        if !advance_source_index(&snapshot).unwrap() {
            break;
        }
    }
    let page = read(&ctx, &snapshot, None, None, None).unwrap();
    assert!(page
        .events
        .iter()
        .any(|row| row.text.as_deref() == Some("usable older prompt")));
    assert_eq!(page.progress, "oversized_record");
    assert!(page.conversation_id.is_none());
    assert!(page.bytes_read < 2 * 1024 * 1024);
}

#[test]
fn artifact_replacement_invalidates_a_body_without_changing_the_preview_or_path() {
    let (_guard, temp) = isolated_home();
    let owner = ProjectionOwner::default();
    let directory = super::super::conversation_dir("agent-1", "conversation").unwrap();
    fs::create_dir_all(directory.join("artifacts")).unwrap();
    let path = directory.join("artifacts/body.txt");
    fs::write(&path, "a".repeat(2000)).unwrap();
    let mut row = event("artifact", "preview");
    row.text = None;
    row.metadata =
        json!({"text_excerpt": "same preview", "text_artifact_refs": ["artifacts/body.txt"]});
    owner
        .committed(candidate(vec![row.clone()]), "one".into())
        .unwrap();
    let first = read(&context(), &snapshot(&context()), None, None, None).unwrap();
    let reference = first.events[0].metadata["chat_detail_ref"]
        .as_str()
        .unwrap();
    let old = read(
        &context(),
        &snapshot(&context()),
        None,
        None,
        Some(reference),
    )
    .unwrap();
    assert_eq!(old.detail.unwrap().text, "a".repeat(2000));
    // Keep the old openable artifact handle intact, then publish a new file at
    // the same relative path. File identity proves the payload changed.
    fs::rename(&path, temp.path().join("old-body.txt")).unwrap();
    fs::write(&path, "b".repeat(2000)).unwrap();
    owner.committed(candidate(vec![row]), "two".into()).unwrap();
    assert!(read(
        &context(),
        &snapshot(&context()),
        None,
        None,
        Some(reference)
    )
    .is_err());
    let changed = read(
        &context(),
        &snapshot(&context()),
        None,
        Some(&first.revision),
        None,
    )
    .unwrap();
    let reference = changed.events[0].metadata["chat_detail_ref"]
        .as_str()
        .unwrap();
    let new = read(
        &context(),
        &snapshot(&context()),
        None,
        None,
        Some(reference),
    )
    .unwrap();
    assert_eq!(new.detail.unwrap().text, "b".repeat(2000));
}
