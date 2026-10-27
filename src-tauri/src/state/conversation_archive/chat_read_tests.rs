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

fn read_complete_detail(
    context: &crate::state::conversation_archive::ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    revision: &str,
    detail_ref: &str,
) -> String {
    read_complete_detail_inner(context, snapshot, revision, detail_ref, None)
}

fn read_complete_detail_for_event(
    context: &crate::state::conversation_archive::ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    revision: &str,
    detail_ref: &str,
    expected_event_id: &str,
) -> String {
    read_complete_detail_inner(
        context,
        snapshot,
        revision,
        detail_ref,
        Some(expected_event_id),
    )
}

fn read_complete_detail_inner(
    context: &crate::state::conversation_archive::ConversationArchiveContext,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    revision: &str,
    detail_ref: &str,
    expected_event_id: Option<&str>,
) -> String {
    let mut next = Some(detail_ref.to_owned());
    let mut body = String::new();
    for _ in 0..16 {
        let page = super::read(context, snapshot, None, Some(revision), next.as_deref()).unwrap();
        let detail = page.detail.expect("the pinned body detail is readable");
        if let Some(expected_event_id) = expected_event_id {
            assert_eq!(
                detail.event_id.as_str(),
                expected_event_id,
                "every body chunk remains bound to its requested source row"
            );
        }
        body.push_str(&detail.text);
        if detail.complete {
            assert!(detail.next.is_none());
            return body;
        }
        next = detail.next;
        assert!(next.is_some(), "an incomplete body exposes its next chunk");
    }
    panic!("bounded body detail chunks did not complete");
}

#[test]
fn large_owned_message_body_requires_and_uses_its_exact_canonical_source_row() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("large-owned-message.jsonl");
    let message_text = "雪🧪".repeat(20_000);
    assert!(message_text.len() > 8 * super::BODY_BYTES);
    assert!(message_text.len() <= 16 * super::BODY_BYTES);
    let message_line = commentary_item_line_with_text("large-owned-message", &message_text);
    assert!(message_line.len() > 128 * 1024);
    assert!(
        message_line.len()
            < crate::commands::provider_log_acquisition::PROVIDER_LOG_MAX_RECORD_BYTES as usize
    );
    assert!(std::str::from_utf8(message_line.as_bytes()).is_ok());
    assert!(serde_json::from_str::<serde_json::Value>(&message_line).is_ok());
    let contents = format!(
        "{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        message_line
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);

    let mut acquired = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut acquired.events,
        "codex",
        &path,
    );
    let source_message = acquired
        .events
        .iter()
        .find(|event| {
            event.kind == AgentChatEventKind::Message
                && event.text.as_deref() == Some(message_text.as_str())
        })
        .expect("the actual native response item decodes as one large message");
    let message_id = source_message.id.clone();
    let message_start = source_message.metadata["chat_source_start"].clone();
    let message_end = source_message.metadata["chat_source_end"].clone();

    finish_source_index(&snapshot);
    let source_page = source_index_page(&snapshot);
    assert!(source_page.counts_complete);
    let settled_pointer = source_page.reference.clone();
    assert!(!super::advance_source_index(&snapshot).unwrap());
    assert_eq!(
        super::super::chat_source_index::pointer("agent-1")
            .unwrap()
            .as_deref(),
        Some(settled_pointer.as_str()),
        "an unchanged complete replay does not churn its published head"
    );
    let empty_archive_page = read(&context, &snapshot, None, None, None).unwrap();
    assert!(empty_archive_page
        .events
        .iter()
        .any(|event| { event.metadata["chat_source_ref"].as_str() == Some(message_id.as_str()) }));
    let missing_body = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&empty_archive_page.revision),
        None,
        Some(std::slice::from_ref(&message_id)),
    )
    .unwrap();
    assert_eq!(missing_body.revalidated_ids, None);
    assert_eq!(missing_body.progress, "body_pending");
    assert!(missing_body
        .events
        .iter()
        .any(|event| { event.metadata["chat_source_ref"].as_str() == Some(message_id.as_str()) }));

    let archive = ConversationArchiveState::default();
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &acquired.events,
            acquired.previous.as_ref(),
            &acquired.next,
            acquired.source_proof(),
        )
        .unwrap();
    finish_projection(&archive);
    let ordinary = read(&context, &snapshot, None, None, None).unwrap();
    let (source_epoch_before, source_admission_before, source_extent_before, _, live_policy_before) =
        crate::commands::chat_recent_seed::source_scope_with_policy(&snapshot).unwrap();
    assert_eq!(
        live_policy_before.generation, acquired.next.policy_generation,
        "canonical publication installs the actual acquisition policy"
    );
    let published = ordinary
        .events
        .iter()
        .find(|event| event.metadata["chat_source_ref"].as_str() == Some(message_id.as_str()))
        .expect("ordinary archive publication retains the physical source reference");
    assert_eq!(published.id, message_id);
    assert!(published.metadata["chat_body_binding"].as_str().is_some());

    let canonical_pointer_before = super::head(&context.agent_id).unwrap();
    let source_pointer_before =
        super::super::chat_source_index::pointer(&context.agent_id).unwrap();
    let canonical_head_before = canonical_pointer_before.as_deref().and_then(|reference| {
        let mut diagnostic_store = super::store(&context.agent_id).ok()?;
        let head = diagnostic_store
            .read::<serde_json::Value>(reference)
            .ok()?;
        Some(json!({
            "agent_id": head.get("agent_id").cloned().unwrap_or(serde_json::Value::Null),
            "source_key_matches_context": head
                .get("source_key")
                .and_then(serde_json::Value::as_str)
                == context.provider_source_key.as_deref(),
            "conversation_id": head.get("conversation_id").cloned().unwrap_or(serde_json::Value::Null),
            "generation": head.get("generation").cloned().unwrap_or(serde_json::Value::Null),
            "source_epoch": head.get("source_epoch").cloned().unwrap_or(serde_json::Value::Null),
            "progress": head.get("progress").cloned().unwrap_or(serde_json::Value::Null),
        }))
    });
    let source_head_before = source_pointer_before.as_deref().and_then(|reference| {
        let mut diagnostic_store = super::store(&context.agent_id).ok()?;
        let head = diagnostic_store
            .read::<serde_json::Value>(reference)
            .ok()?;
        Some(json!({
            "agent_id": head.get("agent_id").cloned().unwrap_or(serde_json::Value::Null),
            "source_key_matches_context": head
                .get("source_key")
                .and_then(serde_json::Value::as_str)
                == context.provider_source_key.as_deref(),
            "generation": head.get("generation").cloned().unwrap_or(serde_json::Value::Null),
            "epoch": head.get("epoch").cloned().unwrap_or(serde_json::Value::Null),
            "admission": head.get("admission").cloned().unwrap_or(serde_json::Value::Null),
            "floor": head.get("floor").cloned().unwrap_or(serde_json::Value::Null),
            "target_extent": head.get("target_extent").cloned().unwrap_or(serde_json::Value::Null),
            "admitted_through": head.get("admitted_through").cloned().unwrap_or(serde_json::Value::Null),
            "progress": head.get("progress").cloned().unwrap_or(serde_json::Value::Null),
            "counts_gapped": head.get("counts_gapped").cloned().unwrap_or(serde_json::Value::Null),
            "replay_settled": head.get("replay_settled").cloned().unwrap_or(serde_json::Value::Null),
        }))
    });
    let stored_source_admission_before = source_head_before
        .as_ref()
        .and_then(|head| head["admission"].as_str())
        .expect("the previously indexed source head stores its admission")
        .to_owned();
    assert_ne!(
        stored_source_admission_before, source_admission_before,
        "the indexed head still carries the prior policy admission"
    );
    let revision_matches_before = super::revision_matches_heads(
        &ordinary.revision,
        canonical_pointer_before.as_deref(),
        source_pointer_before.as_deref(),
    );
    assert!(
        revision_matches_before,
        "the stale ordinary revision still pins both exact published heads"
    );

    assert_eq!(ordinary.progress, "source_changed");
    assert!(ordinary.reset);
    assert!(ordinary.generation.is_none());
    let stale_renewed = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&ordinary.revision),
        None,
        Some(std::slice::from_ref(&message_id)),
    )
    .unwrap();
    assert_eq!(stale_renewed.progress, "source_changed");
    assert!(stale_renewed.events.is_empty());
    assert_eq!(stale_renewed.revalidated_ids, None);

    finish_source_index(&snapshot);
    let reindexed_source_page = source_index_page(&snapshot);
    assert!(reindexed_source_page.counts_complete);
    let (source_epoch_after, source_admission_after, source_extent_after, _, live_policy_after) =
        crate::commands::chat_recent_seed::source_scope_with_policy(&snapshot).unwrap();
    assert_eq!(
        live_policy_after.generation,
        acquired.next.policy_generation
    );
    assert_eq!(reindexed_source_page.epoch, source_epoch_after);
    assert_eq!(reindexed_source_page.admission, source_admission_after);
    assert_eq!(reindexed_source_page.target_extent, source_extent_after);

    let reindexed_ordinary = read(&context, &snapshot, None, None, None).unwrap();
    assert_ne!(reindexed_ordinary.progress, "source_changed");
    assert!(!reindexed_ordinary.reset);
    assert!(reindexed_ordinary.generation.is_some());
    let renewed = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&reindexed_ordinary.revision),
        None,
        Some(std::slice::from_ref(&message_id)),
    )
    .unwrap();
    let indexed_message = source_page
        .events
        .iter()
        .find(|event| event.metadata["chat_source_ref"].as_str() == Some(message_id.as_str()))
        .map(|event| {
            json!({
                "id": event.id,
                "source_ref": event.metadata["chat_source_ref"],
                "source_start": event.metadata["chat_source_start"],
                "source_end": event.metadata["chat_source_end"],
                "detail_ref": event.metadata["chat_detail_ref"],
                "body_binding": event.metadata["chat_body_binding"],
                "body_pending": event.metadata["chat_body_pending"],
                "display_physical_id": event.metadata["chat_display_physical_id"],
                "display_member_ids": event.metadata["chat_display_member_ids"],
            })
        });
    let published_message = json!({
        "id": published.id,
        "source_ref": published.metadata["chat_source_ref"],
        "detail_ref": published.metadata["chat_detail_ref"],
        "body_binding": published.metadata["chat_body_binding"],
        "body_pending": published.metadata["chat_body_pending"],
    });
    let missing_body_rows: Vec<_> = missing_body
        .events
        .iter()
        .filter(|event| event.metadata["chat_source_ref"].as_str() == Some(message_id.as_str()))
        .map(|event| {
            json!({
                "id": event.id,
                "source_ref": event.metadata["chat_source_ref"],
                "detail_ref": event.metadata["chat_detail_ref"],
                "body_binding": event.metadata["chat_body_binding"],
                "body_pending": event.metadata["chat_body_pending"],
            })
        })
        .collect();
    let renewed_rows: Vec<_> = renewed
        .events
        .iter()
        .filter(|event| event.metadata["chat_source_ref"].as_str() == Some(message_id.as_str()))
        .map(|event| {
            json!({
                "id": event.id,
                "source_ref": event.metadata["chat_source_ref"],
                "source_start": event.metadata["chat_source_start"],
                "source_end": event.metadata["chat_source_end"],
                "detail_ref": event.metadata["chat_detail_ref"],
                "body_binding": event.metadata["chat_body_binding"],
                "body_pending": event.metadata["chat_body_pending"],
                "display_physical_id": event.metadata["chat_display_physical_id"],
                "display_member_ids": event.metadata["chat_display_member_ids"],
            })
        })
        .collect();
    let canonical_pointer_after = super::head(&context.agent_id).unwrap();
    let source_pointer_after = super::super::chat_source_index::pointer(&context.agent_id).unwrap();
    let revision_matches_after = super::revision_matches_heads(
        &reindexed_ordinary.revision,
        canonical_pointer_after.as_deref(),
        source_pointer_after.as_deref(),
    );
    assert!(
        revision_matches_after,
        "the positive ordinary revision pins both reindexed heads"
    );
    eprintln!(
        "CHAT1693_LARGE_MESSAGE_REVALIDATION_DIAGNOSTIC {}",
        json!({
            "head_token": {
                "ordinary_revision": &ordinary.revision,
                "canonical_pointer_before": &canonical_pointer_before,
                "source_pointer_before": &source_pointer_before,
                "revision_matches_before": revision_matches_before,
                "canonical_head_before": canonical_head_before,
                "source_head_before": source_head_before,
                "ordinary_page": {
                    "conversation_id": &ordinary.conversation_id,
                    "generation": &ordinary.generation,
                    "source_epoch": &ordinary.source_epoch,
                    "progress": &ordinary.progress,
                    "reset": ordinary.reset,
                },
                "canonical_pointer_after": canonical_pointer_after,
                "source_pointer_after": source_pointer_after,
                "revision_matches_after": revision_matches_after,
                "published_policy_generation": acquired.next.policy_generation,
                "live_policy_before": {
                    "generation": live_policy_before.generation,
                    "epoch": source_epoch_before,
                    "admission": source_admission_before,
                    "extent": source_extent_before,
                },
                "stored_source_admission_before": stored_source_admission_before,
                "live_policy_after": {
                    "generation": live_policy_after.generation,
                    "epoch": source_epoch_after,
                    "admission": source_admission_after,
                    "extent": source_extent_after,
                },
                "reindexed_source_page": {
                    "generation": reindexed_source_page.generation,
                    "epoch": reindexed_source_page.epoch,
                    "admission": reindexed_source_page.admission,
                    "target_extent": reindexed_source_page.target_extent,
                    "counts_complete": reindexed_source_page.counts_complete,
                    "progress": reindexed_source_page.progress,
                },
                "reindexed_ordinary_revision": &reindexed_ordinary.revision,
            },
            "input": {
                "message_id": message_id,
                "raw_record_bytes": message_line.len(),
                "body_utf8_bytes": message_text.len(),
            },
            "source_page": {
                "generation": source_page.generation,
                "epoch": source_page.epoch,
                "admission": source_page.admission,
                "target_extent": source_page.target_extent,
                "counts_complete": source_page.counts_complete,
                "progress": source_page.progress,
                "event_count": source_page.events.len(),
                "matching_row": indexed_message,
            },
            "missing_body": {
                "progress": missing_body.progress,
                "revalidated_ids": missing_body.revalidated_ids,
                "matching_rows": missing_body_rows,
            },
            "stale_renewal": {
                "progress": &stale_renewed.progress,
                "reset": stale_renewed.reset,
                "revalidated_ids": &stale_renewed.revalidated_ids,
                "event_count": stale_renewed.events.len(),
            },
            "published": published_message,
            "renewed": {
                "progress": &renewed.progress,
                "revision": &renewed.revision,
                "conversation_id": &renewed.conversation_id,
                "generation": &renewed.generation,
                "source_epoch": &renewed.source_epoch,
                "reset": renewed.reset,
                "revalidated_ids": &renewed.revalidated_ids,
                "removed_ids": &renewed.removed_ids,
                "matching_rows": renewed_rows,
            },
        })
    );
    assert_eq!(renewed.revalidated_ids, Some(vec![message_id.clone()]));
    let renewed_message = renewed
        .events
        .iter()
        .find(|event| event.id == message_id)
        .expect("the complete current native row is revalidated");
    assert_eq!(
        renewed_message.metadata["chat_source_ref"].as_str(),
        Some(message_id.as_str())
    );
    assert_eq!(renewed_message.metadata["chat_source_start"], message_start);
    assert_eq!(renewed_message.metadata["chat_source_end"], message_end);
    assert!(renewed_message
        .text
        .as_ref()
        .is_some_and(|text| text.len() <= super::PREVIEW_BYTES));
    assert!(renewed_message.metadata["chat_body_binding"]
        .as_str()
        .is_some_and(|binding| !binding.is_empty()));
    assert_eq!(renewed_message.metadata["chat_body_pending"], false);
    let detail_ref = renewed_message.metadata["chat_detail_ref"]
        .as_str()
        .expect("the exact canonical physical body can be loaded");
    assert_eq!(
        read_complete_detail(&context, &snapshot, &renewed.revision, detail_ref),
        message_text
    );
}

#[test]
fn recent_seed_source_end_includes_lf_and_crlf_without_extending_detail_payload() {
    for newline in ["\n", "\r\n"] {
        let (_guard, temp) = isolated_home();
        let path = temp.path().join("recent-seed-framed-source.jsonl");
        let text = "雪".repeat(600);
        let payload = json!({
            "type":"event_msg",
            "payload":{"type":"user_message","id":"framed-message","message":text}
        })
        .to_string();
        let session = json!({"type":"session_meta","payload":{"id":"one"}}).to_string();
        let contents = format!("{session}{newline}{payload}{newline}");
        let (snapshot, _) = owned_native_source(&path, &contents);

        let seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
        let event = seed
            .events
            .iter()
            .find(|event| {
                event.source.as_deref() == Some("event_msg")
                    && event.metadata["raw_type"].as_str() == Some("user_message")
            })
            .expect("the bounded source header for the native message is present");
        assert!(event
            .text
            .as_ref()
            .is_some_and(|header| header.len() <= super::PREVIEW_BYTES));
        assert_ne!(event.text.as_deref(), Some(text.as_str()));
        let start = event.metadata["chat_source_start"].as_u64().unwrap();
        let end = event.metadata["chat_source_end"].as_u64().unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            &bytes[start as usize..end as usize],
            format!("{payload}{newline}").as_bytes()
        );
        let detail_ref = event.metadata["chat_detail_ref"].as_str().unwrap();
        let parts: Vec<_> = detail_ref.split(':').collect();
        assert_eq!(parts[5].parse::<usize>().unwrap(), payload.len());
        assert_eq!(
            parts[6],
            crate::commands::chat_recent_seed::hash(payload.as_bytes())
        );
        let detail = crate::commands::chat_recent_seed::detail(&snapshot, detail_ref).unwrap();
        assert_eq!(detail.text, text);
        assert!(detail.complete);
    }
}

#[test]
fn source_index_resumes_a_partial_suffix_only_after_its_record_is_framed() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("source-index-partial-suffix.jsonl");
    let base = format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line()
    );
    let (snapshot, _) = owned_native_source(&path, &base);
    let message_text = "resumed only after the newline arrives";
    let partial_line = commentary_item_line_with_text("partial-native-message", message_text);
    assert!(serde_json::from_str::<serde_json::Value>(&partial_line).is_ok());

    let mut source_file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut source_file, partial_line.as_bytes()).unwrap();
    drop(source_file);
    let partial_extent = fs::metadata(&path).unwrap().len();
    finish_source_index(&snapshot);
    let partial_page = source_index_page(&snapshot);
    assert!(!partial_page.counts_complete);
    assert_eq!(partial_page.progress, "source_partial_record");
    assert_eq!(partial_page.target_extent, partial_extent);
    let partial_head = partial_page.reference.clone();
    assert!(!super::advance_source_index(&snapshot).unwrap());
    assert_eq!(
        super::super::chat_source_index::pointer("agent-1")
            .unwrap()
            .as_deref(),
        Some(partial_head.as_str()),
        "a pinned partial suffix waits without republishing or busy retry"
    );

    let mut source_file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut source_file, b"\n").unwrap();
    drop(source_file);
    finish_source_index(&snapshot);
    let completed = source_index_page(&snapshot);
    assert!(completed.counts_complete);
    assert_eq!(completed.target_extent, fs::metadata(&path).unwrap().len());
    let resumed = completed
        .events
        .iter()
        .find(|event| event.text.as_deref() == Some(message_text))
        .expect("the completed fixed interval resumes the exact partial record");
    assert_eq!(
        resumed.metadata["chat_source_ref"].as_str(),
        Some(resumed.id.as_str())
    );
}

fn commentary_context_line() -> String {
    json!({"type":"turn_context","payload":{"turn_id":"commentary-provider-turn"}}).to_string()
}

fn commentary_stream_line() -> String {
    json!({
        "timestamp":"2026-10-24T17:38:18.275Z",
        "type":"event_msg",
        "payload":{"type":"agent_message","phase":"commentary","message":"A source-bound commentary observation."}
    })
    .to_string()
}

fn commentary_item_line(id: &str) -> String {
    json!({
        "timestamp":"2026-10-24T17:38:18.275Z",
        "type":"response_item",
        "payload":{
            "type":"message",
            "id":id,
            "role":"assistant",
            "phase":"commentary",
            "content":[{"type":"output_text","text":"A source-bound commentary observation."}],
            "internal_chat_message_metadata_passthrough":{
                "turn_id":"commentary-provider-turn",
                "create_time":"2026-10-24T17:38:18.275Z",
                "content_item_kinds":["unknown"]
            }
        }
    })
    .to_string()
}

fn commentary_stream_line_with_text(text: &str) -> String {
    json!({
        "timestamp":"2026-10-24T17:38:18.275Z",
        "type":"event_msg",
        "payload":{"type":"agent_message","phase":"commentary","message":text}
    })
    .to_string()
}

fn commentary_item_line_with_text(id: &str, text: &str) -> String {
    json!({
        "timestamp":"2026-10-24T17:38:18.275Z",
        "type":"response_item",
        "payload":{
            "type":"message",
            "id":id,
            "role":"assistant",
            "phase":"commentary",
            "content":[{"type":"output_text","text":text}],
            "internal_chat_message_metadata_passthrough":{
                "turn_id":"commentary-provider-turn",
                "create_time":"2026-10-24T17:38:18.275Z",
                "content_item_kinds":["unknown"]
            }
        }
    })
    .to_string()
}

fn finish_source_index(snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot) {
    for _ in 0..128 {
        if !super::advance_source_index(snapshot).unwrap() {
            return;
        }
    }
    panic!("bounded source-index checkpoints did not complete");
}

fn source_index_diagnostic_state(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    before: Option<&str>,
) -> serde_json::Value {
    let current_scope = match crate::commands::chat_recent_seed::source_scope(snapshot) {
        Ok((epoch, admission, extent, modified)) => json!({
            "epoch": epoch,
            "admission": admission,
            "extent": extent,
            "modified": modified,
        }),
        Err(error) => json!({ "error": error.to_string() }),
    };
    match super::super::chat_source_index::pointer("agent-1") {
        Ok(Some(reference)) => {
            let mut store = match super::store("agent-1") {
                Ok(store) => store,
                Err(error) => {
                    return json!({
                        "head_pointer": reference,
                        "current_scope": current_scope,
                        "store_error": error.to_string(),
                    });
                }
            };
            let head = match store.read::<serde_json::Value>(&reference) {
                Ok(head) => json!({
                    "agent_id": head.get("agent_id").cloned().unwrap_or(serde_json::Value::Null),
                    "source_key": head.get("source_key").cloned().unwrap_or(serde_json::Value::Null),
                    "generation": head.get("generation").cloned().unwrap_or(serde_json::Value::Null),
                    "epoch": head.get("epoch").cloned().unwrap_or(serde_json::Value::Null),
                    "admission": head.get("admission").cloned().unwrap_or(serde_json::Value::Null),
                    "root": head.get("root").cloned().unwrap_or(serde_json::Value::Null),
                    "before": head.get("before").cloned().unwrap_or(serde_json::Value::Null),
                    "floor": head.get("floor").cloned().unwrap_or(serde_json::Value::Null),
                    "target_extent": head.get("target_extent").cloned().unwrap_or(serde_json::Value::Null),
                    "progress": head.get("progress").cloned().unwrap_or(serde_json::Value::Null),
                    "counts_gapped": head.get("counts_gapped").cloned().unwrap_or(serde_json::Value::Null),
                    "logical_version": head.get("logical_version").cloned().unwrap_or(serde_json::Value::Null),
                    "replay_settled": head.get("replay_settled").cloned().unwrap_or(serde_json::Value::Null),
                    "replay_bytes_read": head.get("replay_bytes_read").cloned().unwrap_or(serde_json::Value::Null),
                    "replay_records_decoded": head.get("replay_records_decoded").cloned().unwrap_or(serde_json::Value::Null),
                    "replay_rows_admitted": head.get("replay_rows_admitted").cloned().unwrap_or(serde_json::Value::Null),
                }),
                Err(error) => json!({ "read_error": error.to_string() }),
            };
            let page = match super::super::chat_source_index::page(
                snapshot, &mut store, &reference, before, None,
            ) {
                Ok(page) => json!({
                    "counts_complete": page.counts_complete,
                    "before": before,
                    "progress": page.progress,
                    "extent": page.target_extent,
                    "next_before": page.next_before,
                    "generation": page.generation,
                    "epoch": page.epoch,
                    "admission": page.admission,
                    "reset": page.reset,
                }),
                Err(error) => json!({ "read_error": error.to_string() }),
            };
            json!({
                "head_pointer": reference,
                "current_scope": current_scope,
                "head": head,
                "page": page,
            })
        }
        Ok(None) => json!({
            "head_pointer": serde_json::Value::Null,
            "current_scope": current_scope,
            "head": serde_json::Value::Null,
            "page": serde_json::Value::Null,
        }),
        Err(error) => json!({
            "pointer_error": error.to_string(),
            "current_scope": current_scope,
        }),
    }
}

fn finish_source_index_with_diagnostic(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    fixture_stage: &str,
) {
    let mut advance_results = Vec::new();
    let mut checkpoints = vec![json!({
        "after_advance": 0,
        "returned": serde_json::Value::Null,
        "state": source_index_diagnostic_state(snapshot, None),
    })];
    for _ in 0..128 {
        let advanced = super::advance_source_index(snapshot).unwrap();
        advance_results.push(advanced);
        checkpoints.push(json!({
            "after_advance": advance_results.len(),
            "returned": advanced,
            "state": source_index_diagnostic_state(snapshot, None),
        }));
        if !advanced {
            eprintln!(
                "CHAT1693_SOURCE_INDEX_DIAGNOSTIC {}",
                json!({
                    "fixture_stage": fixture_stage,
                    "advance_count": advance_results.len(),
                    "advance_results": advance_results,
                    "checkpoints": checkpoints,
                })
            );
            return;
        }
    }
    eprintln!(
        "CHAT1693_SOURCE_INDEX_DIAGNOSTIC {}",
        json!({
            "fixture_stage": fixture_stage,
            "advance_count": advance_results.len(),
            "advance_results": advance_results,
            "checkpoints": checkpoints,
        })
    );
    panic!("bounded source-index checkpoints did not complete");
}

fn source_index_page(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
) -> super::super::chat_source_index::SourcePage {
    source_index_page_with_cursor(snapshot, None)
}

fn source_index_page_with_cursor(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    cursor: Option<&str>,
) -> super::super::chat_source_index::SourcePage {
    let reference = super::super::chat_source_index::pointer("agent-1")
        .unwrap()
        .unwrap();
    let mut store = super::store("agent-1").unwrap();
    super::super::chat_source_index::page(snapshot, &mut store, &reference, cursor, None).unwrap()
}

#[test]
fn commentary_seed_join_revalidates_full_owned_native_evidence() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("commentary.jsonl");
    let contents = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        commentary_stream_line(),
        commentary_item_line("msg-commentary-native")
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    finish_source_index(&snapshot);
    let mut seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let indexed = source_index_page(&snapshot);
    assert!(indexed.counts_complete);
    let pairs =
        super::complete_seed_commentary_pairs(&context, &snapshot, &seed, Some(&indexed)).unwrap();
    assert_eq!(pairs.pairs.len(), 1);
    let mut pending_index = indexed.clone();
    pending_index.counts_complete = false;
    pending_index.events.clear();
    assert_eq!(
        super::complete_seed_commentary_pairs(&context, &snapshot, &seed, Some(&pending_index))
            .unwrap()
            .pairs
            .len(),
        1,
        "a complete admitted seed uses the logical index count kernel while durable counts are pending"
    );
    let live = seed
        .events
        .iter()
        .position(|event| event.source.as_deref() == Some("event_msg"))
        .unwrap();
    let completed = seed
        .events
        .iter()
        .position(|event| event.source.as_deref() == Some("response_item"))
        .unwrap();

    seed.events[live].metadata["provider_session_id"] = json!("foreign-session");
    assert!(
        super::complete_seed_commentary_pairs(&context, &snapshot, &seed, Some(&indexed)).is_none()
    );
    seed.events[live].metadata["provider_session_id"] = json!("one");

    // Admission permits absent optional session metadata; the header and source
    // coordinates still bind both observations to this native source.
    seed.events[live]
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("provider_session_id");
    seed.events[completed]
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("provider_session_id");
    assert_eq!(
        super::complete_seed_commentary_pairs(&context, &snapshot, &seed, Some(&indexed))
            .unwrap()
            .pairs
            .len(),
        1
    );
}

#[test]
fn source_ownership_is_independent_from_narrative_message_eligibility() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("source-ownership.jsonl");
    let contents = format!(
        "{}\n{}\n{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        native_tool_call_line("owned-tool-call"),
        commentary_context_line(),
        commentary_stream_line(),
        commentary_item_line("msg-commentary-native")
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let (epoch, admission_id, extent, _) =
        crate::commands::chat_recent_seed::source_scope(&snapshot).unwrap();
    let log_path = fs::canonicalize(&path)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let mut tool_call = seed
        .events
        .iter()
        .find(|event| event.kind == AgentChatEventKind::ToolCall)
        .unwrap()
        .clone();
    tool_call.metadata["log_path"] = json!(log_path);
    let message = seed
        .events
        .iter()
        .find(|event| {
            event.kind == AgentChatEventKind::Message
                && event.role == Some(AgentChatRole::Assistant)
                && event.metadata["provider_phase"] == "commentary"
        })
        .unwrap();
    let source_admission = super::super::chat_logical_index::Admission {
        context: &context,
        conversation: None,
        epoch: &epoch,
        admission: &admission_id,
        path: &log_path,
        watermark: extent,
        sequence_trusted: false,
    };

    assert!(source_admission.native(&tool_call));
    assert_eq!(
        tool_call.metadata["chat_source_admission"].as_str(),
        Some(admission_id.as_str())
    );
    assert!(source_admission.source_owned(&tool_call));
    assert!(
        !source_admission.owned(&tool_call),
        "a native ToolCall is source-owned but is not a narrative message"
    );

    assert!(source_admission.native(message));
    assert!(source_admission.owned(message));
    let mut foreign_message = (*message).clone();
    foreign_message.metadata["provider_session_id"] = json!("foreign-session");
    assert!(source_admission.native(&foreign_message));
    assert!(!source_admission.source_owned(&foreign_message));
    assert!(
        !source_admission.owned(&foreign_message),
        "eligible messages still require current provider-session ownership"
    );
    let mut roleless_message = (*message).clone();
    roleless_message.role = None;
    assert!(source_admission.source_owned(&roleless_message));
    assert!(!source_admission.owned(&roleless_message));
}

#[test]
fn empty_owned_source_revision_reaches_the_first_published_header() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("empty-owned-initialization.jsonl");
    let session = json!({"type":"session_meta","payload":{"id":"one"}}).to_string();
    fs::write(&path, format!("{session}\n")).unwrap();

    let mut snapshot = snapshot(&context());
    snapshot.provider = "codex".into();
    snapshot.resume_session = Some("one".into());
    snapshot.log_path = Some(path.clone());
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();

    let empty_capture = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    assert!(empty_capture.events.is_empty());
    assert_eq!(
        empty_capture.next.committed_offset,
        fs::metadata(&path).unwrap().len()
    );
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &empty_capture.events,
            empty_capture.previous.as_ref(),
            &empty_capture.next,
            empty_capture.source_proof(),
        )
        .unwrap();
    finish_source_index(&snapshot);
    assert!(
        source_index_page(&snapshot).counts_complete,
        "the empty native-source checkpoint is complete before the first owned header"
    );
    assert!(
        super::head("agent-1").unwrap().is_none(),
        "native source-index setup does not publish a canonical conversation"
    );

    let empty = read(&context, &snapshot, None, None, None).unwrap();
    let empty_request = json!({
        "cursor": null,
        "revision": null,
        "detail_ref": null,
        "revalidate_ids": null,
    });
    eprintln!(
        "CHAT_EMPTY_OWNED_ACTUAL label=empty request={} dto={}",
        serde_json::to_string(&empty_request).unwrap(),
        serde_json::to_string(&empty).unwrap()
    );
    assert!(empty.events.is_empty());
    assert!(empty.conversation_id.is_none());
    assert!(super::head("agent-1").unwrap().is_none());
    let earlier_revision = empty.revision.clone();
    append_native_line(
        &path,
        native_message("first owned native header").trim_end(),
    );
    let mut first_owned = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(empty_capture.next.clone()),
        true,
    )
    .unwrap();
    assert_eq!(first_owned.events.len(), 1);
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut first_owned.events,
        "codex",
        &path,
    );
    let acquired = first_owned.events[0].clone();
    assert_eq!(acquired.kind, AgentChatEventKind::Message);
    assert_eq!(acquired.text.as_deref(), Some("first owned native header"));

    assert_eq!(
        archive
            .append_verified_provider_log_batch_with_context(
                context.clone(),
                &first_owned.events,
                first_owned.previous.as_ref(),
                &first_owned.next,
                first_owned.source_proof(),
            )
            .unwrap(),
        1
    );
    finish_projection(&archive);
    finish_source_index(&snapshot);
    assert!(super::head("agent-1").unwrap().is_some());

    let proof = super::super::chat_logical_index::source_proof(&context)
        .unwrap()
        .expect("the ordinary provider capture publishes its current source proof");
    let ownership = super::super::chat_logical_index::Admission {
        context: &context,
        conversation: None,
        epoch: &proof.epoch,
        admission: &proof.admission,
        path: &proof.path,
        watermark: proof.watermark,
        sequence_trusted: proof.sequence_trusted,
    };
    assert!(ownership.native(&acquired));
    assert!(ownership.source_owned(&acquired));
    assert!(ownership.owned(&acquired));

    // Reuse the exact opaque revision from the empty response. The ordinary
    // poll has no cursor, detail request or revalidation cohort.
    let request = json!({
        "cursor": null,
        "revision": earlier_revision,
        "detail_ref": null,
        "revalidate_ids": null,
    });
    let first_page = read(&context, &snapshot, None, Some(&earlier_revision), None).unwrap();
    eprintln!(
        "CHAT_EMPTY_OWNED_ACTUAL label=first-owned-after-empty-revision request={} dto={}",
        serde_json::to_string(&request).unwrap(),
        serde_json::to_string(&first_page).unwrap()
    );
    assert_eq!(first_page.events.len(), 1);
    let displayed = &first_page.events[0];
    assert_eq!(displayed.id, acquired.id);
    assert_eq!(displayed.session_id, context.agent_id);
    assert_eq!(displayed.kind, AgentChatEventKind::Message);
    assert_eq!(displayed.text.as_deref(), Some("first owned native header"));
    assert_eq!(
        displayed.metadata["chat_source_ref"].as_str(),
        Some(acquired.id.as_str())
    );
    assert_eq!(
        displayed.metadata["chat_source_epoch"].as_str(),
        Some(proof.epoch.as_str())
    );
    if let Some(displayed_admission) = displayed.metadata["chat_source_admission"].as_str() {
        assert_eq!(displayed_admission, proof.admission.as_str());
    }

    let fresh_request = json!({
        "cursor": null,
        "revision": null,
        "detail_ref": null,
        "revalidate_ids": null,
    });
    let fresh_page = read(&context, &snapshot, None, None, None).unwrap();
    eprintln!(
        "CHAT_EMPTY_OWNED_ACTUAL label=first-owned-after-empty-fresh-none request={} dto={}",
        serde_json::to_string(&fresh_request).unwrap(),
        serde_json::to_string(&fresh_page).unwrap()
    );
    assert_eq!(fresh_page.events.len(), 1);
    let fresh_displayed = &fresh_page.events[0];
    assert_eq!(fresh_displayed.id, acquired.id);
    assert_eq!(fresh_displayed.session_id, context.agent_id);
    assert_eq!(fresh_displayed.kind, AgentChatEventKind::Message);
    assert_eq!(
        fresh_displayed.text.as_deref(),
        Some("first owned native header")
    );
    assert_eq!(
        fresh_displayed.metadata["chat_source_ref"].as_str(),
        Some(acquired.id.as_str())
    );
    assert_eq!(
        fresh_displayed.metadata["chat_source_epoch"].as_str(),
        Some(proof.epoch.as_str())
    );
    if let Some(displayed_admission) = fresh_displayed.metadata["chat_source_admission"].as_str() {
        assert_eq!(displayed_admission, proof.admission.as_str());
    }
}

#[test]
fn large_native_tool_output_keeps_source_coverage_separate_from_narrative_pairs() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("large-owned-tool-output.jsonl");
    let pair_text = "A source-bound commentary observation.";
    let output_text = "雪🧪".repeat(20_000);
    assert!(output_text.len() > 8 * super::BODY_BYTES);
    assert!(output_text.len() <= 16 * super::BODY_BYTES);
    let output_record = json!({
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call_output",
            "call_id": "large-native-output",
            "output": output_text.clone(),
        }
    });
    let output_line = serde_json::to_string(&output_record).unwrap();
    let output_record_bytes = output_line.len();
    assert!(output_record_bytes > 128 * 1024);
    assert!(
        output_record_bytes
            < crate::commands::provider_log_acquisition::PROVIDER_LOG_MAX_RECORD_BYTES as usize
    );
    assert!(std::str::from_utf8(output_line.as_bytes()).is_ok());
    assert!(serde_json::from_str::<serde_json::Value>(&output_line).is_ok());

    let mut contents = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}})
    );
    for record in [
        commentary_context_line(),
        native_tool_call_line("large-native-output"),
        output_line.clone(),
        commentary_stream_line_with_text(pair_text),
        commentary_item_line_with_text("msg-commentary-native", pair_text),
    ] {
        contents.push_str(&record);
        contents.push('\n');
    }
    fs::write(&path, &contents).unwrap();
    assert!(std::str::from_utf8(&fs::read(&path).unwrap()).is_ok());

    let mut snapshot = snapshot(&context());
    snapshot.provider = "codex".into();
    snapshot.resume_session = Some("one".into());
    snapshot.log_path = Some(path.clone());
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut acquired = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut acquired.events,
        "codex",
        &path,
    );

    let mut capture_cursor = serde_json::to_value(&acquired.next).unwrap();
    let normalizer = capture_cursor
        .get_mut("normalizer")
        .and_then(serde_json::Value::as_object_mut)
        .expect("capture cursor serializes its actual normalizer state");
    let pending_event_count = normalizer
        .get("pending_events")
        .and_then(serde_json::Value::as_array)
        .map_or(0, |events| events.len());
    normalizer.insert(
        "pending_events".into(),
        json!({"redacted_count": pending_event_count}),
    );
    let capture_events = acquired
        .events
        .iter()
        .map(|event| {
            json!({
                "id": event.id,
                "kind": format!("{:?}", event.kind),
                "role": event.role.as_ref().map(|role| format!("{role:?}")),
                "source": event.source,
                "turn_id": event.turn_id,
                "provider_turn_id": event.metadata.get("provider_turn_id"),
                "raw_type": event.metadata.get("raw_type"),
                "metadata_keys": event.metadata.as_object().map(|metadata| metadata.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                "text_bytes": event.text.as_ref().map(String::len),
                "source_ref": event.metadata.get("chat_source_ref"),
                "source_start": event.metadata.get("chat_source_start"),
                "source_end": event.metadata.get("chat_source_end"),
            })
        })
        .collect::<Vec<_>>();
    eprintln!(
        "CHAT_LARGE_TOOL_OUTPUT_ACTUAL stage=provider-capture {}",
        json!({
            "record_bytes": output_record_bytes,
            "record_utf8": std::str::from_utf8(output_line.as_bytes()).is_ok(),
            "record_json": serde_json::from_str::<serde_json::Value>(&output_line).is_ok(),
            "capture_cursor": capture_cursor,
            "events": capture_events,
        })
    );

    assert_eq!(acquired.next.status, "complete");
    assert_eq!(
        acquired.next.committed_offset,
        fs::metadata(&path).unwrap().len()
    );
    assert_eq!(
        acquired
            .events
            .iter()
            .filter(|event| {
                event.kind == AgentChatEventKind::ToolCall
                    && event.source.as_deref() == Some("response_item")
                    && event.metadata["raw_type"] == "custom_tool_call"
            })
            .count(),
        1,
        "the actual Codex custom-tool-call envelope accompanies its output"
    );
    let tool_outputs = acquired
        .events
        .iter()
        .filter(|event| {
            event.kind == AgentChatEventKind::ToolResult
                && event.source.as_deref() == Some("response_item")
                && event.metadata["raw_type"] == "custom_tool_call_output"
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_outputs.len(),
        1,
        "the capture has one decoded large ToolResult"
    );
    let tool_output = tool_outputs[0];
    assert_eq!(tool_output.role, Some(AgentChatRole::Tool));
    assert!(tool_output.text.is_some());
    let tool_output_id = tool_output.id.clone();
    let live_id = acquired
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("event_msg") && event.text.as_deref() == Some(pair_text)
        })
        .unwrap()
        .id
        .clone();
    let completed_id = acquired
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("response_item")
                && event.text.as_deref() == Some(pair_text)
        })
        .unwrap()
        .id
        .clone();
    assert_ne!(live_id, completed_id);

    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &acquired.events,
            acquired.previous.as_ref(),
            &acquired.next,
            acquired.source_proof(),
        )
        .unwrap();
    finish_projection(&archive);
    finish_source_index(&snapshot);

    let ordinary = read(&context, &snapshot, None, None, None).unwrap();
    let source_page = source_index_page(&snapshot);
    let archived = archive.chat_events_for_agent("agent-1").unwrap();
    let current_proof = super::super::chat_logical_index::source_proof(&context);
    let (proof_summary, tool_ownership) = match current_proof {
        Ok(Some(proof)) => {
            let admission = super::super::chat_logical_index::Admission {
                context: &context,
                conversation: None,
                epoch: &proof.epoch,
                admission: &proof.admission,
                path: &proof.path,
                watermark: proof.watermark,
                sequence_trusted: proof.sequence_trusted,
            };
            (
                json!({
                    "epoch": proof.epoch,
                    "admission": proof.admission,
                    "watermark": proof.watermark,
                    "narrative_complete": proof.narrative_complete,
                    "complete_prefix": proof.complete_prefix,
                }),
                Some((
                    admission.native(tool_output),
                    admission.source_owned(tool_output),
                    admission.owned(tool_output),
                )),
            )
        }
        Ok(None) => (serde_json::Value::Null, None),
        Err(error) => (json!({"error": error.to_string()}), None),
    };
    let summarize_event = |event: &AgentChatEvent| {
        json!({
            "id": event.id,
            "kind": format!("{:?}", &event.kind),
            "role": event.role.as_ref().map(|role| format!("{role:?}")),
            "source": event.source,
            "text_bytes": event.text.as_ref().map(String::len),
            "source_ref": event.metadata.get("chat_source_ref").cloned().unwrap_or(serde_json::Value::Null),
            "source_start": event.metadata.get("chat_source_start").cloned().unwrap_or(serde_json::Value::Null),
            "source_end": event.metadata.get("chat_source_end").cloned().unwrap_or(serde_json::Value::Null),
            "source_epoch": event.metadata.get("chat_source_epoch").cloned().unwrap_or(serde_json::Value::Null),
            "source_admission": event.metadata.get("chat_source_admission").cloned().unwrap_or(serde_json::Value::Null),
            "physical_id": event.metadata.get("chat_display_physical_id").cloned().unwrap_or(serde_json::Value::Null),
            "member_ids": event.metadata.get("chat_display_member_ids").cloned().unwrap_or(serde_json::Value::Null),
        })
    };
    let tracked = |event: &AgentChatEvent| {
        event.id == live_id
            || event.id == completed_id
            || event.id == tool_output_id
            || [
                live_id.as_str(),
                completed_id.as_str(),
                tool_output_id.as_str(),
            ]
            .contains(
                &event.metadata["chat_source_ref"]
                    .as_str()
                    .unwrap_or_default(),
            )
    };
    let archived_rows = archived
        .iter()
        .filter(|event| tracked(event))
        .map(summarize_event)
        .collect::<Vec<_>>();
    let displayed_rows = ordinary
        .events
        .iter()
        .filter(|event| tracked(event))
        .map(summarize_event)
        .collect::<Vec<_>>();
    let source_rows = source_page
        .events
        .iter()
        .filter(|event| tracked(event))
        .map(summarize_event)
        .collect::<Vec<_>>();
    let canonical_head = match super::head("agent-1").unwrap() {
        Some(reference) => {
            let mut store = super::store("agent-1").unwrap();
            match store.read::<super::Head>(&reference) {
                Ok(head) => json!({
                    "reference": reference,
                    "conversation_id": head.conversation_id,
                    "generation": head.generation,
                    "source_epoch": head.source_epoch,
                    "committed_output_bytes": head.committed_output_bytes,
                    "progress": head.progress,
                }),
                Err(error) => json!({"reference": reference, "error": error.to_string()}),
            }
        }
        None => serde_json::Value::Null,
    };
    let index_state = source_index_diagnostic_state(&snapshot, None);
    eprintln!(
        "CHAT_LARGE_TOOL_OUTPUT_ACTUAL stage=current-head-and-source-index {}",
        json!({
            "tool_record_bytes": output_record_bytes,
            "provider_capture": {
                "status": acquired.next.status,
                "committed_offset": acquired.next.committed_offset,
                "events": acquired.events.iter().map(summarize_event).collect::<Vec<_>>(),
            },
            "durable_candidate_rows": archived_rows,
            "canonical_head": canonical_head,
            "ordinary_page": {
                "revision": ordinary.revision,
                "conversation_id": ordinary.conversation_id,
                "generation": ordinary.generation,
                "source_epoch": ordinary.source_epoch,
                "reset": ordinary.reset,
                "progress": ordinary.progress,
                "next_before": ordinary.next_before,
                "rows": displayed_rows,
            },
            "current_source_proof": proof_summary,
            "tool_output_owner": tool_ownership,
            "source_index_page": {
                "counts_complete": source_page.counts_complete,
                "reference": source_page.reference,
                "progress": source_page.progress,
                "target_extent": source_page.target_extent,
                "generation": source_page.generation,
                "epoch": source_page.epoch,
                "admission": source_page.admission,
                "next_before": source_page.next_before,
                "reset": source_page.reset,
                "rows": source_rows,
            },
            "source_index_state": index_state,
        })
    );

    assert_eq!(tool_ownership, Some((true, true, false)));
    assert!(ordinary.events.iter().any(|event| event.id == live_id));
    let ordinary_pair = ordinary
        .events
        .iter()
        .find(|event| event.id == live_id)
        .expect("the canonical projection keeps public A as the pair slot");
    assert_eq!(
        ordinary_pair.metadata["chat_display_physical_id"].as_str(),
        Some(completed_id.as_str())
    );
    let ordinary_members: BTreeSet<_> = ordinary_pair.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|member| member.as_str())
        .collect();
    assert_eq!(
        ordinary_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    assert!(!ordinary.events.iter().any(|event| event.id == completed_id));

    let renewed_result = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&ordinary.revision),
        None,
        Some(std::slice::from_ref(&live_id)),
    );
    let renewed_summary = match &renewed_result {
        Ok(page) => json!({
            "ok": true,
            "revision": page.revision.clone(),
            "conversation_id": page.conversation_id.clone(),
            "generation": page.generation.clone(),
            "source_epoch": page.source_epoch.clone(),
            "reset": page.reset,
            "progress": page.progress.clone(),
            "next_before": page.next_before.clone(),
            "event_count": page.events.len(),
            "revalidated_ids": page.revalidated_ids.clone(),
            "rows": page.events.iter().filter(|event| tracked(event)).map(summarize_event).collect::<Vec<_>>(),
        }),
        Err(error) => json!({
            "ok": false,
            "error": error.to_string(),
        }),
    };
    eprintln!(
        "CHAT_LARGE_TOOL_OUTPUT_ACTUAL stage=a-only-source-revalidation {}",
        json!({
            "request": {
                "revision": ordinary.revision.clone(),
                "cursor": null,
                "detail_ref": null,
                "revalidate_ids": [live_id.clone()],
            },
            "response": renewed_summary,
        })
    );

    assert!(
        source_page.counts_complete,
        "the valid owner-native ToolResult must not be confused with an unowned narrative row; inspect CHAT_LARGE_TOOL_OUTPUT_ACTUAL for independent source-index gap evidence"
    );
    let tool_body_page = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&ordinary.revision),
        None,
        Some(std::slice::from_ref(&tool_output_id)),
    )
    .unwrap();
    assert_eq!(
        tool_body_page.revalidated_ids,
        Some(vec![tool_output_id.clone()])
    );
    let tool_body_header = tool_body_page
        .events
        .iter()
        .find(|event| event.metadata["chat_source_ref"].as_str() == Some(tool_output_id.as_str()))
        .expect("the current physical ToolResult row is returned");
    assert_eq!(tool_body_header.id, tool_output_id);
    assert_eq!(tool_body_header.kind, AgentChatEventKind::ToolResult);
    assert!(tool_body_header
        .text
        .as_ref()
        .is_some_and(|text| text.len() <= super::PREVIEW_BYTES));
    assert!(tool_body_header.metadata["chat_body_binding"]
        .as_str()
        .is_some_and(|binding| !binding.is_empty()));
    assert_eq!(tool_body_header.metadata["chat_body_pending"], false);
    let tool_detail_ref = tool_body_header.metadata["chat_detail_ref"]
        .as_str()
        .expect("body metadata points to the current canonical physical row");
    assert_eq!(
        read_complete_detail(
            &context,
            &snapshot,
            &tool_body_page.revision,
            tool_detail_ref
        ),
        output_text
    );

    let renewed = renewed_result.unwrap();
    assert_eq!(renewed.revalidated_ids, Some(vec![live_id.clone()]));
    let displayed_pair = renewed
        .events
        .iter()
        .find(|event| event.id == live_id)
        .expect("A-only revalidation follows the current native relation to B");
    assert_eq!(
        displayed_pair.metadata["chat_display_physical_id"].as_str(),
        Some(completed_id.as_str())
    );
    let members: BTreeSet<_> = displayed_pair.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|member| member.as_str())
        .collect();
    assert_eq!(
        members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );

    let late_line = commentary_item_line_with_text("msg-commentary-late-C", pair_text);
    append_native_line(&path, &late_line);
    let mut late = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(acquired.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut late.events, "codex", &path);
    assert!(late.events.iter().any(|event| {
        event.source.as_deref() == Some("response_item") && event.text.as_deref() == Some(pair_text)
    }));
    archive
        .append_verified_provider_log_batch_with_context(
            context.clone(),
            &late.events,
            late.previous.as_ref(),
            &late.next,
            late.source_proof(),
        )
        .unwrap();
    finish_projection(&archive);
    finish_source_index(&snapshot);
    let after_late_index = source_index_page(&snapshot);
    assert!(
        after_late_index.counts_complete,
        "late C must be evaluated against a complete current source index"
    );
    let after_late = read(&context, &snapshot, None, Some(&renewed.revision), None).unwrap();
    let pair_ids = vec![live_id.clone(), completed_id.clone()];
    let raw_pair = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&after_late.revision),
        None,
        Some(&pair_ids),
    )
    .unwrap();
    eprintln!(
        "CHAT_LARGE_TOOL_OUTPUT_ACTUAL stage=late-C requested_ids={} source_index={}",
        serde_json::to_string(&pair_ids).unwrap(),
        json!({
            "counts_complete": after_late_index.counts_complete,
            "next_before": after_late_index.next_before,
            "progress": after_late_index.progress,
            "target_extent": after_late_index.target_extent,
            "generation": after_late_index.generation,
            "epoch": after_late_index.epoch,
            "admission": after_late_index.admission,
            "revalidated_ids": raw_pair.revalidated_ids,
            "rows": raw_pair.events.iter().map(summarize_event).collect::<Vec<_>>(),
        })
    );
    assert_eq!(raw_pair.revalidated_ids, Some(pair_ids));
    assert_eq!(
        raw_pair
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    for source_id in [&live_id, &completed_id] {
        let raw = raw_pair
            .events
            .iter()
            .find(|event| &event.id == source_id)
            .unwrap();
        let acquired = acquired
            .events
            .iter()
            .find(|event| &event.id == source_id)
            .unwrap();
        assert_eq!(
            raw.metadata["chat_source_ref"].as_str(),
            Some(source_id.as_str())
        );
        assert_eq!(
            raw.metadata["chat_source_start"],
            acquired.metadata["chat_source_start"]
        );
        assert_eq!(
            raw.metadata["chat_source_end"],
            acquired.metadata["chat_source_end"]
        );
        assert_eq!(raw.text, acquired.text);
        assert!(raw_pair.aliases.iter().any(|alias| {
            alias.observation_id == *source_id && alias.canonical_id == *source_id
        }));
        assert!(!raw_pair.removed_ids.contains(source_id));
    }
}

#[test]
fn source_index_replays_legacy_tool_call_coverage_gaps() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("legacy-tool-call-coverage.jsonl");
    let contents = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        native_tool_call_line("older-tool-call"),
        native_tool_call_line("newer-tool-call"),
        commentary_context_line(),
        commentary_stream_line(),
        commentary_item_line("msg-commentary-native")
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    finish_source_index(&snapshot);

    let current = source_index_page(&snapshot);
    assert!(current.counts_complete);
    let generation = current.generation.clone();
    let epoch = current.epoch.clone();
    let admission = current.admission.clone();
    let extent = current.target_extent;
    let source_bytes = fs::read(&path).unwrap();
    assert_eq!(source_bytes, contents.as_bytes());
    let source_rows = |page: &super::super::chat_source_index::SourcePage| {
        page.events
            .iter()
            .map(|event| {
                (
                    event.metadata["chat_display_physical_id"]
                        .as_str()
                        .unwrap_or(event.id.as_str())
                        .to_owned(),
                    event.metadata["chat_source_ref"]
                        .as_str()
                        .map(str::to_owned),
                    event.metadata["chat_source_start"].as_u64(),
                    event.metadata["chat_source_end"].as_u64(),
                    event.metadata["chat_source_epoch"]
                        .as_str()
                        .map(str::to_owned),
                    event.metadata["chat_source_admission"]
                        .as_str()
                        .map(str::to_owned),
                    format!("{:?}", event.kind),
                    format!("{:?}", event.role),
                    event.source.clone(),
                    event.turn_id.clone(),
                    event.text.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let current_source_rows = source_rows(&current);
    assert!(current.events.iter().all(|event| {
        event.metadata["chat_source_admission"].as_str() == Some(admission.as_str())
    }));
    let source_ref = super::super::chat_source_index::pointer("agent-1")
        .unwrap()
        .unwrap();
    let mut store = super::store("agent-1").unwrap();
    let mut legacy_head: serde_json::Value = store.read(&source_ref).unwrap();
    assert!(legacy_head["before"].is_null());
    assert_eq!(legacy_head["logical_version"], 5);
    assert_eq!(legacy_head["counts_gapped"], false);
    assert_eq!(legacy_head["replay_settled"], true);
    assert!(
        legacy_head["replay_records_decoded"].as_u64().unwrap() >= contents.lines().count() as u64
    );
    assert!(legacy_head["replay_bytes_read"].as_u64().unwrap() >= source_bytes.len() as u64);
    assert!(legacy_head["replay_rows_admitted"].as_u64().unwrap() > 0);
    legacy_head["logical_version"] = json!(2);
    legacy_head["counts_gapped"] = json!(true);
    assert_eq!(legacy_head["logical_version"], 2);
    assert_eq!(legacy_head["counts_gapped"], true);
    let legacy_ref = store.put(&legacy_head).unwrap();
    let pointer_path = super::locations("agent-1")
        .unwrap()
        .0
        .with_file_name("chat-source-head.json");
    wardian_core::conversations::write_json_atomic(&pointer_path, &Some(legacy_ref)).unwrap();

    let stale = source_index_page(&snapshot);
    assert!(
        !stale.counts_complete,
        "a legacy head is never accepted before its bounded replay"
    );
    assert!(
        stale
            .events
            .iter()
            .all(|event| event.metadata["chat_display_member_ids"].is_null()),
        "an unsupported source-index version cannot project a saved relation"
    );
    finish_source_index(&snapshot);
    let requalified = source_index_page(&snapshot);
    assert!(requalified.counts_complete);
    assert_eq!(requalified.generation, generation);
    assert_eq!(requalified.epoch, epoch);
    assert_eq!(requalified.admission, admission);
    assert_eq!(requalified.target_extent, extent);
    assert_eq!(source_rows(&requalified), current_source_rows);
    assert_eq!(fs::read(&path).unwrap(), source_bytes);
    assert_eq!(
        requalified
            .events
            .iter()
            .filter(|event| event.kind == AgentChatEventKind::ToolCall)
            .count(),
        2,
        "bounded requalification preserves native nonnarrative source rows"
    );
    assert!(requalified.events.iter().any(|event| {
        event.metadata["chat_display_member_ids"]
            .as_array()
            .is_some_and(|members| members.len() == 2)
    }));

    // A real bounded-record gap discovered during legacy replay must remain
    // uncertified after the old sticky bit is cleared and the index is rebuilt.
    let over_limit_record =
        vec![
            b'x';
            crate::commands::provider_log_acquisition::PROVIDER_LOG_MAX_RECORD_BYTES as usize + 1
        ];
    let mut source_file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut source_file, &over_limit_record).unwrap();
    drop(source_file);
    let source_ref = super::super::chat_source_index::pointer("agent-1")
        .unwrap()
        .unwrap();
    let mut store = super::store("agent-1").unwrap();
    let mut legacy_head: serde_json::Value = store.read(&source_ref).unwrap();
    assert!(legacy_head["before"].is_null());
    assert_eq!(legacy_head["logical_version"], 5);
    assert_eq!(legacy_head["counts_gapped"], false);
    assert_eq!(legacy_head["replay_settled"], true);
    assert!(
        legacy_head["replay_records_decoded"].as_u64().unwrap() >= contents.lines().count() as u64
    );
    assert!(legacy_head["replay_bytes_read"].as_u64().unwrap() >= source_bytes.len() as u64);
    assert!(legacy_head["replay_rows_admitted"].as_u64().unwrap() > 0);
    assert_eq!(legacy_head["target_extent"].as_u64(), Some(extent));
    legacy_head["logical_version"] = json!(2);
    legacy_head["counts_gapped"] = json!(true);
    assert_eq!(legacy_head["logical_version"], 2);
    assert_eq!(legacy_head["counts_gapped"], true);
    let legacy_ref = store.put(&legacy_head).unwrap();
    wardian_core::conversations::write_json_atomic(&pointer_path, &Some(legacy_ref)).unwrap();

    assert!(!source_index_page(&snapshot).counts_complete);
    finish_source_index(&snapshot);
    let still_gapped = source_index_page(&snapshot);
    assert!(!still_gapped.counts_complete);
    assert_eq!(still_gapped.progress, "oversized_record");
    assert_eq!(still_gapped.generation, generation);
    assert_eq!(still_gapped.epoch, epoch);
    assert_eq!(still_gapped.admission, admission);
    assert_eq!(source_rows(&still_gapped), current_source_rows);
    let rebuilt_ref = super::super::chat_source_index::pointer("agent-1")
        .unwrap()
        .unwrap();
    let mut store = super::store("agent-1").unwrap();
    let rebuilt_head: serde_json::Value = store.read(&rebuilt_ref).unwrap();
    assert_eq!(rebuilt_head["logical_version"], 5);
    assert_eq!(
        rebuilt_head["before"].as_u64(),
        rebuilt_head["target_extent"].as_u64(),
        "the over-limit suffix remains outside the certified replay prefix"
    );
    assert_eq!(rebuilt_head["counts_gapped"], true);
    assert_eq!(rebuilt_head["replay_settled"], false);
    assert_eq!(
        rebuilt_head["target_extent"].as_u64(),
        Some(fs::metadata(&path).unwrap().len())
    );
    assert!(fs::read(&path).unwrap().starts_with(&source_bytes));
}

#[test]
fn bounded_commentary_identity_keeps_full_turn_and_source_path_evidence() {
    let (_guard, temp) = isolated_home();
    let prefix = "turn-identity-".repeat(24);
    let live_turn = format!("{prefix}live");
    let item_turn = format!("{prefix}item");
    assert_eq!(&live_turn[..256], &item_turn[..256]);
    let text = "same visible commentary";
    let contents = format!(
        "{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"event_msg","payload":{"type":"agent_message","phase":"commentary","message":text,"internal_chat_message_metadata_passthrough":{"turn_id":live_turn,"chat_display_identity_sha256":"f".repeat(64)}}}),
        json!({"type":"response_item","payload":{"type":"message","id":"commentary-item","role":"assistant","phase":"commentary","text":text,"internal_chat_message_metadata_passthrough":{"turn_id":item_turn,"chat_display_identity_sha256":"f".repeat(64)}}})
    );
    let (snapshot, _) = owned_native_source(&temp.path().join("long-turns.jsonl"), &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    finish_source_index(&snapshot);
    let page = read(&context, &snapshot, None, None, None).unwrap();
    let rows: Vec<_> = page
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some(text))
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|event| event.metadata["chat_display_member_ids"].is_null()));
    assert!(rows.iter().all(|event| event.metadata
        [crate::providers::chat_transcript::CODEX_DISPLAY_IDENTITY_SHA256]
        .is_null()));
    assert!(!page.aliases.iter().any(|alias| {
        rows.iter().any(|event| event.id == alias.canonical_id)
            && rows.iter().any(|event| event.id == alias.observation_id)
            && alias.canonical_id != alias.observation_id
    }));

    // This source format does not currently populate source_path. Exercise the
    // same bounded-header handoff with full, distinct paths that share a preview.
    let path = temp.path().join("same-turn.jsonl");
    let paired_contents = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        commentary_stream_line(),
        commentary_item_line("path-item")
    );
    let (snapshot, _) = owned_native_source(&path, &paired_contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    finish_source_index(&snapshot);
    let mut seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let indexed = source_index_page(&snapshot);
    assert_eq!(
        super::complete_seed_commentary_pairs(&context, &snapshot, &seed, Some(&indexed))
            .unwrap()
            .pairs
            .len(),
        1
    );
    let path_prefix = "p".repeat(256);
    for (event, suffix) in
        seed.events
            .iter_mut()
            .filter_map(|event| match event.source.as_deref() {
                Some("event_msg") => Some((event, "-live")),
                Some("response_item") => Some((event, "-item")),
                _ => None,
            })
    {
        let mut full = event.clone();
        full.metadata["source_path"] = json!(format!("{path_prefix}{suffix}"));
        full.metadata[crate::providers::chat_transcript::CODEX_DISPLAY_IDENTITY_SHA256] =
            json!(crate::providers::chat_transcript::codex_display_identity_sha256(&full).unwrap());
        *event = header(&full);
    }
    let live = seed
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("event_msg"))
        .unwrap();
    let item = seed
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("response_item"))
        .unwrap();
    assert_eq!(live.metadata["source_path"], item.metadata["source_path"]);
    assert_ne!(
        live.metadata[crate::providers::chat_transcript::CODEX_DISPLAY_IDENTITY_SHA256],
        item.metadata[crate::providers::chat_transcript::CODEX_DISPLAY_IDENTITY_SHA256]
    );
    assert!(
        super::complete_seed_commentary_pairs(&context, &snapshot, &seed, Some(&indexed))
            .unwrap()
            .pairs
            .is_empty()
    );
}

#[test]
fn source_index_identity_uses_full_commentary_text_before_header_clipping() {
    let (_guard, temp) = isolated_home();
    let preview = "p".repeat(PREVIEW_BYTES);
    let matching_text = format!("{preview} matching tail");
    let live_tail_text = format!("{preview} live tail");
    let final_tail_text = format!("{preview} final tail");
    assert!(matching_text.len() > PREVIEW_BYTES);
    assert!(live_tail_text.len() > PREVIEW_BYTES);
    assert!(final_tail_text.len() > PREVIEW_BYTES);
    assert_ne!(live_tail_text, final_tail_text);
    assert_eq!(
        &live_tail_text[..PREVIEW_BYTES],
        &final_tail_text[..PREVIEW_BYTES],
        "the native rows differ only after their visible header preview"
    );

    let contents = [
        json!({"type":"session_meta","payload":{"id":"one"}}).to_string(),
        json!({"type":"turn_context","payload":{"turn_id":"matching-turn"}}).to_string(),
        json!({
            "type":"event_msg",
            "payload":{"type":"agent_message","phase":"commentary","message":matching_text}
        })
        .to_string(),
        json!({
            "type":"response_item",
            "payload":{
                "type":"message",
                "id":"matching-final",
                "role":"assistant",
                "phase":"commentary",
                "content":[{"type":"output_text","text":matching_text}],
                "internal_chat_message_metadata_passthrough":{
                    "turn_id":"matching-turn",
                    "create_time":"2026-10-24T17:38:18.275Z",
                    "content_item_kinds":["unknown"]
                }
            }
        })
        .to_string(),
        json!({"type":"turn_context","payload":{"turn_id":"tail-collision-turn"}}).to_string(),
        json!({
            "type":"event_msg",
            "payload":{"type":"agent_message","phase":"commentary","message":live_tail_text}
        })
        .to_string(),
        json!({
            "type":"response_item",
            "payload":{
                "type":"message",
                "id":"tail-collision-final",
                "role":"assistant",
                "phase":"commentary",
                "content":[{"type":"output_text","text":final_tail_text}],
                "internal_chat_message_metadata_passthrough":{
                    "turn_id":"tail-collision-turn",
                    "create_time":"2026-10-24T17:38:18.275Z",
                    "content_item_kinds":["unknown"]
                }
            }
        })
        .to_string(),
    ]
    .join("\n")
        + "\n";
    let path = temp
        .path()
        .join("source-index-full-commentary-identity.jsonl");
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let mut acquired = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut acquired.events,
        "codex",
        &path,
    );
    assert_eq!(acquired.next.status, "complete");

    let matching_live = acquired
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("event_msg")
                && event.text.as_deref() == Some(matching_text.as_str())
        })
        .unwrap();
    let matching_final = acquired
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("response_item")
                && event.text.as_deref() == Some(matching_text.as_str())
        })
        .unwrap();
    let live_tail = acquired
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("event_msg")
                && event.text.as_deref() == Some(live_tail_text.as_str())
        })
        .unwrap();
    let final_tail = acquired
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("response_item")
                && event.text.as_deref() == Some(final_tail_text.as_str())
        })
        .unwrap();
    for (event, expected) in [
        (matching_live, matching_text.as_str()),
        (matching_final, matching_text.as_str()),
        (live_tail, live_tail_text.as_str()),
        (final_tail, final_tail_text.as_str()),
    ] {
        assert_eq!(event.text.as_deref(), Some(expected));
        assert!(event.text.as_ref().unwrap().len() > PREVIEW_BYTES);
        assert!(event.metadata["chat_source_ref"].as_str().is_some());
        assert!(event.metadata["chat_source_start"].as_u64().is_some());
        assert!(event.metadata["chat_source_end"].as_u64().is_some());
    }

    let matching_live_id = matching_live.metadata["chat_source_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let matching_final_id = matching_final.metadata["chat_source_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let live_tail_id = live_tail.metadata["chat_source_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let final_tail_id = final_tail.metadata["chat_source_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(matching_live_id, matching_final_id);
    assert_ne!(live_tail_id, final_tail_id);

    finish_source_index(&snapshot);
    let indexed = source_index_page(&snapshot);
    assert!(indexed.counts_complete);

    let matching_live_row = indexed
        .events
        .iter()
        .find(|event| event.metadata["chat_source_ref"].as_str() == Some(matching_live_id.as_str()))
        .unwrap();
    let matching_final_row = indexed
        .events
        .iter()
        .find(|event| {
            event.metadata["chat_source_ref"].as_str() == Some(matching_final_id.as_str())
        })
        .unwrap();
    assert_eq!(matching_live_row.id, matching_live_id);
    assert_eq!(matching_final_row.id, matching_live_id);
    assert_eq!(matching_live_row.text.as_deref(), Some(preview.as_str()));
    assert_eq!(matching_final_row.text.as_deref(), Some(preview.as_str()));
    assert_eq!(
        matching_live_row.metadata["chat_display_physical_id"].as_str(),
        Some(matching_live_id.as_str())
    );
    assert_eq!(
        matching_final_row.metadata["chat_display_physical_id"].as_str(),
        Some(matching_final_id.as_str())
    );
    let matching_members: std::collections::BTreeSet<_> = matching_final_row.metadata
        ["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        matching_members,
        std::collections::BTreeSet::from([matching_live_id.as_str(), matching_final_id.as_str()])
    );

    let live_tail_row = indexed
        .events
        .iter()
        .find(|event| event.metadata["chat_source_ref"].as_str() == Some(live_tail_id.as_str()))
        .unwrap();
    let final_tail_row = indexed
        .events
        .iter()
        .find(|event| event.metadata["chat_source_ref"].as_str() == Some(final_tail_id.as_str()))
        .unwrap();
    assert_eq!(live_tail_row.text, final_tail_row.text);
    assert_eq!(live_tail_row.text.as_deref(), Some(preview.as_str()));
    assert_eq!(live_tail_row.id, live_tail_id);
    assert_eq!(final_tail_row.id, final_tail_id);
    assert!(live_tail_row.metadata["chat_display_member_ids"].is_null());
    assert!(final_tail_row.metadata["chat_display_member_ids"].is_null());
    assert!(live_tail_row.metadata["chat_display_physical_id"].is_null());
    assert!(final_tail_row.metadata["chat_display_physical_id"].is_null());
}

#[test]
fn commentary_pair_stays_distinct_when_an_earlier_admitted_duplicate_is_outside_page() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("earlier-commentary.jsonl");
    let mut contents = format!(
        "{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        commentary_item_line("msg-earlier-duplicate")
    );
    for index in 0..90 {
        contents.push_str(&native_message(&format!("unrelated-{index}")));
    }
    contents.push_str(&commentary_context_line());
    contents.push('\n');
    contents.push_str(&commentary_stream_line());
    contents.push('\n');
    contents.push_str(&commentary_item_line("msg-commentary-native"));
    contents.push('\n');
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let earlier_raw = contents
        .lines()
        .find(|line| line.contains("msg-earlier-duplicate"))
        .unwrap();
    let earlier_offset = contents.find(earlier_raw).unwrap() as u64;
    let identity = crate::commands::provider_log_acquisition::native_file_identity(
        &File::open(&path).unwrap(),
    )
    .unwrap();
    let earlier_id = crate::commands::chat_recent_seed::source_reference(
        "agent-1",
        &identity,
        earlier_offset,
        earlier_raw,
        0,
    );
    finish_source_index(&snapshot);

    let page = read(&context, &snapshot, None, None, None).unwrap();
    assert!(!page.events.iter().any(|event| event.id == earlier_id));
    let pair: Vec<_> = page
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("A source-bound commentary observation."))
        .collect();
    assert_eq!(pair.len(), 2);
    assert_ne!(pair[0].id, pair[1].id);
    assert!(pair
        .iter()
        .all(|event| event.metadata["chat_display_member_ids"].is_null()));
    assert!(!page
        .aliases
        .iter()
        .any(|alias| alias.canonical_id == pair[0].id && alias.observation_id == pair[1].id));
}

fn native_message(text: &str) -> String {
    format!(
        "{}\n",
        json!({"type": "event_msg", "payload": {"type": "user_message", "message": text}})
    )
}

fn native_tool_call_line(id: &str) -> String {
    json!({
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call",
            "name": "shell_command",
            "call_id": id,
            "input": {"command": "true"}
        }
    })
    .to_string()
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
fn saved_codex_unqualified_saved_ordinal_retry_retains_old_and_native_rows() {
    saved_codex_ambiguous_retry_page(false, false);
}

#[test]
fn saved_codex_unknown_prefix_retry_retains_old_and_both_native_rows() {
    saved_codex_ambiguous_retry_page(true, false);
}

#[test]
fn saved_codex_tail_relative_cursor_collision_never_claims_an_earlier_native_row() {
    saved_codex_ambiguous_retry_page(false, true);
}

/// Retain historical and physical observations when old ordinal origin is unproved.
/// Retry and cold admission must not guess a legacy alias from its numeric cursor.
fn saved_codex_ambiguous_retry_page(oversized: bool, tail_collision: bool) {
    use std::io::Write;
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("saved-codex.jsonl");
    let mut prefix = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}})
    );
    if oversized {
        let filler = format!("{}\n", json!({"type":"ignored","padding":"x".repeat(4096)}));
        for _ in 0..600 {
            prefix.push_str(&filler);
        }
        assert!(prefix.len() > 2 * 1024 * 1024);
    }
    fs::write(&path, &prefix).unwrap();
    let mut snap = snapshot(&context());
    snap.provider = "codex".into();
    snap.resume_session = Some("one".into());
    snap.log_path = Some(path.clone());
    let ctx = crate::commands::chat::conversation_archive_context_from_snapshot(&snap);
    let source_key = ctx.provider_source_key.as_deref().unwrap();
    let archive = ConversationArchiveState::default();
    let initial = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1", "codex", &path, source_key, None, !oversized,
    )
    .unwrap();
    assert!(initial.events.is_empty());
    assert_eq!(initial.next.unknown_before_offset.is_some(), oversized);
    archive
        .append_provider_log_batch_with_context(ctx.clone(), &initial.events, None, &initial.next)
        .unwrap();
    let raw = json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Original saved reply"}]}}).to_string();
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{raw}").unwrap();
    let ignored = json!({"type":"ignored_control"}).to_string();
    if tail_collision {
        writeln!(file, "{ignored}").unwrap();
    }
    let two_native = oversized || tail_collision;
    if two_native {
        writeln!(file, "{raw}").unwrap();
    }
    file.sync_all().unwrap();
    drop(file);
    let mut batch = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        source_key,
        Some(initial.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut batch.events, "codex", &path);
    assert_eq!(batch.events.len(), if two_native { 2 } else { 1 });
    let native_ids: Vec<_> = batch.events.iter().map(|event| event.id.clone()).collect();
    if two_native {
        assert_ne!(native_ids[0], native_ids[1]);
        assert_eq!(
            batch.events[0].metadata["chat_compatibility_legacy_id"],
            batch.events[1].metadata["chat_compatibility_legacy_id"]
        );
    }
    let legacy_id = batch.events[0].metadata["chat_compatibility_legacy_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut legacy = batch.events[if tail_collision { 1 } else { 0 }].clone();
    if tail_collision {
        // This actual tail normalizer starts on the ignored row immediately
        // before the later answer. Its ordinal collides with the earlier answer.
        let tail = format!("{ignored}\n{raw}\n");
        let tail_event = crate::providers::chat_transcript::normalize_chat_lines(
            "agent-1",
            "codex",
            tail.lines(),
        )
        .into_iter()
        .find(|event| event.kind == AgentChatEventKind::Message)
        .unwrap();
        assert_eq!(tail_event.sequence, batch.events[0].sequence);
        assert_ne!(tail_event.sequence, batch.events[1].sequence);
        legacy.sequence = tail_event.sequence;
    }
    legacy.id = legacy_id.clone();
    legacy.metadata.as_object_mut().unwrap().retain(|key, _| {
        !key.starts_with("chat_source_")
            && !key.starts_with("chat_compatibility_")
            && key != "chat_legacy_source_sequence"
            && key != "legacy_event_ids"
            && key != "provider_log_row_offset"
    });
    assert!(legacy.sequence.is_some());
    assert!(legacy.metadata.get("chat_legacy_source_sequence").is_none());
    assert!(legacy.metadata.get("chat_source_ref").is_none());
    assert!(legacy.metadata.get("cursor").is_none());
    let expected_legacy_cursor = legacy.sequence.unwrap().to_string();
    archive
        .append_chat_events_with_context(ctx.clone(), &[legacy])
        .unwrap();
    let saved = archive.chat_events_for_agent("agent-1").unwrap();
    assert!(saved
        .iter()
        .find(|event| event.id == legacy_id)
        .unwrap()
        .metadata
        .get("chat_legacy_source_sequence")
        .is_none());
    let conversation = archive.active_conversation_id("agent-1").unwrap().unwrap();
    let directory = wardian_core::paths::agent_conversation_dir("agent-1", &conversation).unwrap();
    let records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
        wardian_core::conversations::read_jsonl_records(&directory.join("conversation.jsonl"))
            .unwrap();
    let old_record = records
        .iter()
        .find(|record| record.event_refs.contains(&legacy_id))
        .unwrap();
    let sources: Vec<wardian_core::conversations::ConversationSourceRecord> =
        wardian_core::conversations::read_jsonl_records(&directory.join("sources.jsonl")).unwrap();
    let old_source = sources
        .iter()
        .find(|source| {
            source.source_id.starts_with("src_")
                && old_record.source_refs.contains(&source.source_id)
        })
        .unwrap();
    assert_eq!(
        old_source.cursor.as_deref(),
        Some(expected_legacy_cursor.as_str())
    );
    assert!(old_source.offset.is_none());
    archive
        .fail_next_chat_cursor_commit
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failure = archive
        .append_provider_log_batch_with_context(
            ctx.clone(),
            &batch.events,
            batch.previous.as_ref(),
            &batch.next,
        )
        .unwrap_err();
    assert!(failure
        .to_string()
        .contains("injected chat cursor commit failure"));
    assert_eq!(
        archive
            .provider_log_capture_state("agent-1", source_key)
            .unwrap(),
        Some(initial.next.clone())
    );
    drop(archive);
    let reopened = ConversationArchiveState::default();
    let mut replay = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        source_key,
        Some(initial.next),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut replay.events, "codex", &path);
    assert_eq!(
        replay
            .events
            .iter()
            .map(|event| &event.id)
            .collect::<Vec<_>>(),
        native_ids.iter().collect::<Vec<_>>()
    );
    reopened
        .append_provider_log_batch_with_context(
            ctx.clone(),
            &replay.events,
            replay.previous.as_ref(),
            &replay.next,
        )
        .unwrap();
    assert_eq!(
        reopened
            .provider_log_capture_state("agent-1", source_key)
            .unwrap(),
        Some(replay.next.clone())
    );
    drop(reopened);
    // Exercise actual saved-row bootstrap, not a warm candidate or live-tail loader.
    cold_saved_remove_projection(temp.path());
    let mut ready = false;
    for _ in 0..128 {
        if !ConversationArchiveState::default()
            .bootstrap_saved_chat(&ctx)
            .unwrap()
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "cold saved archive admission did not settle");
    let mut foreign = ctx.clone();
    foreign.provider_session_ids = vec!["foreign-session".into()];
    assert!(super::super::chat_logical_index::source_proof(&foreign)
        .unwrap()
        .is_none());
    let page = read(&ctx, &snap, None, None, None).unwrap();
    let rows: Vec<_> = page
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("Original saved reply"))
        .collect();
    assert_eq!(
        rows.len(),
        if two_native { 3 } else { 2 },
        "ambiguous old origin must preserve history and every native occurrence"
    );
    assert!(rows.iter().any(|event| event.id == legacy_id));
    for native_id in &native_ids {
        assert!(rows.iter().any(|event| &event.id == native_id));
        assert!(
            !page
                .aliases
                .iter()
                .any(|alias| &alias.observation_id == native_id && alias.canonical_id == legacy_id),
            "an untagged old cursor cannot transfer physical ownership"
        );
    }
    assert_eq!(
        ConversationArchiveState::default()
            .provider_log_capture_state("agent-1", source_key)
            .unwrap(),
        Some(replay.next)
    );
    assert!(page.records_decoded < 512 && page.bytes_read < 2 * 1024 * 1024);
    let restarted = read(&ctx, &snap, None, None, None).unwrap();
    assert_eq!(
        restarted
            .events
            .iter()
            .map(|event| &event.id)
            .collect::<Vec<_>>(),
        page.events
            .iter()
            .map(|event| &event.id)
            .collect::<Vec<_>>()
    );
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
        crate::commands::chat::archive_identity::legacy_provider_log_event_id(&legacy, &path);
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
    let request = batch
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("response_item"))
        .unwrap();
    assert_eq!(
        request.metadata["chat_compatibility_legacy_id"],
        json!(legacy.id)
    );
    assert_eq!(
        request.metadata["chat_compatibility_source_sequence"],
        legacy.metadata["chat_legacy_source_sequence"]
    );
    let proof = super::super::chat_logical_index::source_proof(&ctx)
        .unwrap()
        .expect("normal capture must qualify the original source");
    assert!(proof.sequence_trusted && proof.narrative_complete);
    assert_eq!(
        rows.len(),
        1,
        "qualified row identities and members: {:?}",
        rows.iter()
            .map(|event| (&event.id, event.metadata["chat_display_member_ids"].clone()))
            .collect::<Vec<_>>()
    );
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

#[test]
fn commentary_relation_retracts_on_late_duplicate_and_reopens_all_native_rows() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("commentary-reopen.jsonl");
    let contents = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        commentary_stream_line(),
        commentary_item_line("msg-commentary-native")
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    let live_id = first
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("event_msg"))
        .unwrap()
        .id
        .clone();
    let completed_id = first
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("response_item"))
        .unwrap()
        .id
        .clone();
    archive
        .append_provider_log_batch_with_context(context.clone(), &first.events, None, &first.next)
        .unwrap();
    finish_projection(&archive);
    finish_source_index(&snapshot);

    let qualified = read(&context, &snapshot, None, None, None).unwrap();
    assert_eq!(qualified.events.len(), 1);
    assert_eq!(qualified.events[0].id, live_id);
    let members: BTreeSet<_> = qualified.events[0].metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );

    // A missing logical-version field represents a persisted clipped-header
    // index. Replaying the existing bounded source index restores its ownership
    // evidence without changing the native coordinates or displayed identity.
    let source_ref = super::super::chat_source_index::pointer("agent-1")
        .unwrap()
        .unwrap();
    let mut store = super::store("agent-1").unwrap();
    let mut old_head: serde_json::Value = store.read(&source_ref).unwrap();
    old_head.as_object_mut().unwrap().remove("logical_version");
    let old_ref = store.put(&old_head).unwrap();
    let pointer_path = super::locations("agent-1")
        .unwrap()
        .0
        .with_file_name("chat-source-head.json");
    wardian_core::conversations::write_json_atomic(&pointer_path, &Some(old_ref)).unwrap();
    finish_source_index(&snapshot);
    let requalified = read(&context, &snapshot, None, Some(&qualified.revision), None).unwrap();

    append_native_line(
        &path,
        &commentary_item_line("msg-commentary-late-duplicate"),
    );
    let mut next = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(first.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut next.events, "codex", &path);
    let late_id = next
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("response_item"))
        .unwrap()
        .id
        .clone();
    // The native log now has C, while the canonical capture and durable source
    // index still contain only A/B. The ordinary page must retract that stale
    // relation from the complete admitted seed before the next capture pass.

    let retracted = read(&context, &snapshot, None, Some(&requalified.revision), None).unwrap();
    assert!(retracted.reset);
    let ids: BTreeSet<_> = retracted
        .events
        .iter()
        .map(|event| event.id.as_str())
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from([live_id.as_str(), completed_id.as_str(), late_id.as_str()])
    );
    assert!(retracted
        .events
        .iter()
        .all(|event| event.metadata["chat_display_member_ids"].is_null()));
    assert!(retracted.aliases.iter().any(|alias| {
        alias.observation_id == completed_id && alias.canonical_id == completed_id
    }));
    assert!(retracted
        .aliases
        .iter()
        .any(|alias| alias.observation_id == live_id && alias.canonical_id == live_id));
    assert!(retracted
        .aliases
        .iter()
        .any(|alias| alias.observation_id == late_id && alias.canonical_id == late_id));
    assert!(!retracted
        .aliases
        .iter()
        .any(|alias| { alias.observation_id == completed_id && alias.canonical_id == live_id }));
    assert!(!retracted.removed_ids.contains(&live_id));
    let archived = archive.chat_events_for_agent("agent-1").unwrap();
    let archived_ids: BTreeSet<_> = archived
        .iter()
        .filter(|event| event.kind == AgentChatEventKind::Message)
        .map(|event| event.id.as_str())
        .collect();
    assert_eq!(
        archived_ids,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );

    drop(archive);
    let reopened = read(&context, &snapshot, None, None, None).unwrap();
    let reopened_ids: BTreeSet<_> = reopened
        .events
        .iter()
        .map(|event| event.id.as_str())
        .collect();
    assert_eq!(reopened_ids, ids);
    assert!(reopened
        .events
        .iter()
        .all(|event| event.metadata["chat_display_member_ids"].is_null()));
    for observation_id in [&live_id, &completed_id, &late_id] {
        assert!(reopened.aliases.iter().any(|alias| {
            alias.observation_id == *observation_id && alias.canonical_id == *observation_id
        }));
    }

    // Replaying the source-index version invalidates the prior projection, so
    // the ordinary reader returns a fresh bounded page with the same qualified
    // visible identity and aliases.
    assert!(requalified.reset);
    assert!(!requalified.events.is_empty());
    assert!(!requalified.unchanged);
    assert_ne!(requalified.revision, qualified.revision);
    assert_eq!(requalified.conversation_id, qualified.conversation_id);
    assert_eq!(requalified.generation, qualified.generation);
    assert_eq!(requalified.source_epoch, qualified.source_epoch);
    assert_eq!(requalified.next_before, qualified.next_before);
    assert_eq!(requalified.events.len(), 1);
    assert_eq!(requalified.events[0].id, live_id);
    let requalified_members: BTreeSet<_> = requalified.events[0].metadata
        ["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        requalified_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    assert!(requalified
        .aliases
        .iter()
        .any(|alias| { alias.observation_id == completed_id && alias.canonical_id == live_id }));
    assert!(!requalified.removed_ids.contains(&live_id));
}

#[test]
fn bounded_seed_retracts_a_pair_only_after_observing_a_same_side_collision() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("bounded-commentary-reopen.jsonl");
    let mut records = vec![json!({"type":"session_meta","payload":{"id":"one"}}).to_string()];
    records.extend((0..84).map(|index| {
        json!({
            "type":"event_msg",
            "payload":{
                "type":"user_message",
                "message":format!("unrelated history {index}")
            }
        })
        .to_string()
    }));
    records.extend([
        commentary_context_line(),
        commentary_stream_line(),
        commentary_item_line("msg-commentary-native"),
    ]);
    let contents = format!("{}\n", records.join("\n"));
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    let live_id = first
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("event_msg")
                && event.metadata["raw_type"] == "agent_message"
                && event.metadata["provider_phase"] == "commentary"
        })
        .unwrap()
        .id
        .clone();
    let completed_id = first
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("response_item")
                && event.metadata["raw_type"] == "message"
                && event.metadata["provider_phase"] == "commentary"
        })
        .unwrap()
        .id
        .clone();
    archive
        .append_provider_log_batch_with_context(context.clone(), &first.events, None, &first.next)
        .unwrap();
    finish_projection(&archive);
    finish_source_index(&snapshot);

    let h0_seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let h0_checkpoint = h0_seed.checkpoint.as_ref().unwrap();
    assert!(h0_checkpoint.before.is_some());
    assert!(h0_checkpoint.normalizer_complete);
    assert!(h0_seed.events.iter().any(|event| event.id == live_id));
    assert!(h0_seed.events.iter().any(|event| event.id == completed_id));
    let epoch = h0_checkpoint.epoch.clone();
    let admission = h0_checkpoint.admission.clone();
    let h0_source_page = source_index_page(&snapshot);
    assert!(h0_source_page.counts_complete);
    assert_eq!(h0_source_page.epoch, epoch);
    assert_eq!(h0_source_page.admission, admission);
    assert_eq!(
        h0_source_page.target_extent,
        fs::metadata(&path).unwrap().len()
    );

    let qualified = read(&context, &snapshot, None, None, None).unwrap();
    let qualified_pair = qualified
        .events
        .iter()
        .find(|event| event.id == live_id)
        .unwrap();
    let qualified_members: BTreeSet<_> = qualified_pair.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        qualified_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    for observation_id in [&live_id, &completed_id] {
        assert!(qualified.aliases.iter().any(|alias| {
            alias.observation_id == *observation_id && alias.canonical_id == live_id
        }));
    }

    let h0_extent = h0_checkpoint.extent;
    let unrelated_text = "unrelated append after the qualified pair";
    append_native_line(
        &path,
        &json!({
            "type":"event_msg",
            "payload":{"type":"user_message","message":unrelated_text}
        })
        .to_string(),
    );
    let h1_seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let h1_checkpoint = h1_seed.checkpoint.as_ref().unwrap();
    assert!(h1_checkpoint.before.is_some());
    assert!(h1_checkpoint.normalizer_complete);
    assert_eq!(h1_checkpoint.epoch, epoch);
    assert_eq!(h1_checkpoint.admission, admission);
    assert!(h1_seed.events.iter().any(|event| event.id == live_id));
    assert!(h1_seed.events.iter().any(|event| event.id == completed_id));
    let unrelated = h1_seed
        .events
        .iter()
        .find(|event| {
            event.metadata["chat_source_start"]
                .as_u64()
                .is_some_and(|start| start >= h0_extent)
                && event.text.as_deref() == Some(unrelated_text)
        })
        .unwrap();
    assert!(unrelated.metadata["chat_source_ref"].as_str().is_some());
    let h1_source_page = source_index_page(&snapshot);
    assert!(!h1_source_page.counts_complete);
    assert!(h1_source_page.target_extent < fs::metadata(&path).unwrap().len());

    let retained = read(&context, &snapshot, None, Some(&qualified.revision), None).unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "bounded-seed-unrelated-H1",
        &retained,
        &live_id,
        &completed_id,
    );
    let retained_pair = retained
        .events
        .iter()
        .find(|event| event.id == live_id)
        .unwrap();
    let retained_members: BTreeSet<_> = retained_pair.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        retained_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    assert!(retained
        .aliases
        .iter()
        .any(|alias| { alias.observation_id == completed_id && alias.canonical_id == live_id }));
    assert!(!retained.removed_ids.contains(&live_id));

    append_native_line(
        &path,
        &commentary_item_line("msg-commentary-late-duplicate"),
    );
    let h2_seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let h2_checkpoint = h2_seed.checkpoint.as_ref().unwrap();
    assert!(h2_checkpoint.before.is_some());
    assert!(h2_checkpoint.normalizer_complete);
    assert_eq!(h2_checkpoint.epoch, epoch);
    assert_eq!(h2_checkpoint.admission, admission);
    assert!(h2_seed.events.iter().any(|event| event.id == live_id));
    let late_id = h2_seed
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("response_item")
                && event.metadata["raw_type"] == "message"
                && event.metadata["chat_source_start"]
                    .as_u64()
                    .is_some_and(|start| start >= h1_checkpoint.extent)
        })
        .unwrap()
        .id
        .clone();
    let completed = h2_seed
        .events
        .iter()
        .find(|event| event.id == completed_id)
        .unwrap();
    let late = h2_seed
        .events
        .iter()
        .find(|event| event.id == late_id)
        .unwrap();
    let completed_evidence =
        crate::providers::chat_transcript::codex_display_evidence_from_bounded_header(completed)
            .unwrap();
    let late_evidence =
        crate::providers::chat_transcript::codex_display_evidence_from_bounded_header(late)
            .unwrap();
    assert_eq!(completed_evidence, late_evidence);
    let h2_source_page = source_index_page(&snapshot);
    assert!(!h2_source_page.counts_complete);
    assert!(h2_source_page.target_extent < fs::metadata(&path).unwrap().len());

    let retracted = read(&context, &snapshot, None, Some(&retained.revision), None).unwrap();
    let visible_ids: BTreeSet<_> = retracted
        .events
        .iter()
        .map(|event| event.id.as_str())
        .collect();
    assert!(visible_ids.contains(live_id.as_str()));
    assert!(visible_ids.contains(completed_id.as_str()));
    assert!(visible_ids.contains(late_id.as_str()));
    assert!(retracted.events.iter().all(|event| {
        event.metadata["chat_display_member_ids"]
            .as_array()
            .is_none_or(|members| {
                !members.iter().filter_map(|id| id.as_str()).any(|member| {
                    member == live_id.as_str()
                        || member == completed_id.as_str()
                        || member == late_id.as_str()
                })
            })
    }));
    for observation_id in [&live_id, &completed_id, &late_id] {
        assert!(retracted.aliases.iter().any(|alias| {
            alias.observation_id == *observation_id && alias.canonical_id == *observation_id
        }));
    }
    assert!(!retracted
        .aliases
        .iter()
        .any(|alias| alias.observation_id == completed_id && alias.canonical_id == live_id));
    assert!(!retracted.removed_ids.contains(&live_id));
}

#[test]
fn stale_logical_head_reconciles_current_qualified_commentary_seed_before_projection() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("stale-commentary-head.jsonl");
    let contents = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        commentary_stream_line(),
        commentary_item_line("msg-commentary-native")
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    let live_id = first
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("event_msg"))
        .unwrap()
        .id
        .clone();
    let completed_id = first
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("response_item"))
        .unwrap()
        .id
        .clone();
    archive
        .append_provider_log_batch_with_context(context.clone(), &first.events, None, &first.next)
        .unwrap();
    finish_projection(&archive);
    finish_source_index(&snapshot);

    let qualified = read(&context, &snapshot, None, None, None).unwrap();
    assert_eq!(qualified.events.len(), 1);
    assert_eq!(qualified.events[0].id, live_id);
    let old_extent = fs::metadata(&path).unwrap().len();
    let old_head_ref = super::head("agent-1").unwrap().unwrap();
    let old_head: super::Head = super::store("agent-1")
        .unwrap()
        .read(&old_head_ref)
        .unwrap();
    assert!(old_head.logical.has_evidence());
    assert_eq!(old_head.logical.completed_watermark(), Some(old_extent));

    let control_text = "A separate native control observation.";
    append_native_line(
        &path,
        &json!({"type":"event_msg","payload":{"type":"user_message","id":"msg-stale-head-control","client_id":"stale-head-control","message":control_text}})
            .to_string(),
    );
    let current_extent = fs::metadata(&path).unwrap().len();
    assert!(current_extent > old_extent);
    finish_source_index(&snapshot);
    assert_eq!(
        super::head("agent-1").unwrap().as_deref(),
        Some(old_head_ref.as_str())
    );
    let stale_head: super::Head = super::store("agent-1")
        .unwrap()
        .read(&old_head_ref)
        .unwrap();
    assert_eq!(stale_head.logical.completed_watermark(), Some(old_extent));

    let current_seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let checkpoint = current_seed.checkpoint.as_ref().unwrap();
    assert_eq!(current_seed.progress, "provisional");
    assert_eq!(current_seed.events.len(), 3);
    assert_eq!(checkpoint.before, None);
    assert!(checkpoint.window_complete);
    assert!(checkpoint.normalizer_complete);
    assert_eq!(checkpoint.extent, current_extent);
    assert!(current_seed
        .events
        .iter()
        .all(|event| event.metadata["chat_source_ref"].as_str() == Some(event.id.as_str())));
    let seed_commentary_ids: BTreeSet<_> = current_seed
        .events
        .iter()
        .filter(|event| event.metadata["provider_phase"].as_str() == Some("commentary"))
        .map(|event| event.id.as_str())
        .collect();
    assert_eq!(
        seed_commentary_ids,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    let control = current_seed
        .events
        .iter()
        .find(|event| event.text.as_deref() == Some(control_text))
        .unwrap();
    assert_eq!(control.role, Some(AgentChatRole::User));
    assert_eq!(control.source.as_deref(), Some("event_msg"));
    assert_eq!(control.metadata["raw_type"].as_str(), Some("user_message"));
    let seed_pairs =
        super::complete_seed_commentary_pairs(&context, &snapshot, &current_seed, None)
            .expect("the complete current seed independently proves the commentary pair");
    assert_eq!(
        seed_pairs.pairs,
        vec![(live_id.clone(), completed_id.clone())]
    );

    let indexed = source_index_page(&snapshot);
    assert!(indexed.counts_complete);
    let indexed_pair = indexed
        .events
        .iter()
        .find(|event| event.metadata["chat_display_member_ids"].is_array())
        .unwrap();
    assert_eq!(indexed_pair.id, live_id);
    let indexed_members: BTreeSet<_> = indexed_pair.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        indexed_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );

    let refreshed = read(&context, &snapshot, None, Some(&qualified.revision), None).unwrap();
    assert_eq!(refreshed.events.len(), 2);
    let commentary = refreshed
        .events
        .iter()
        .find(|event| event.text.as_deref() == Some("A source-bound commentary observation."))
        .unwrap();
    assert_eq!(commentary.id, live_id);
    assert_eq!(
        commentary.metadata["chat_display_physical_id"],
        json!(completed_id.clone())
    );
    assert_eq!(
        commentary.metadata["chat_source_ref"].as_str(),
        Some(completed_id.as_str())
    );
    assert_eq!(commentary.source.as_deref(), Some("response_item"));
    let refreshed_members: BTreeSet<_> = commentary.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        refreshed_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    assert!(refreshed
        .aliases
        .iter()
        .any(|alias| { alias.observation_id == completed_id && alias.canonical_id == live_id }));
    assert!(!refreshed.removed_ids.contains(&live_id));
    assert!(refreshed
        .events
        .iter()
        .any(|event| event.text.as_deref() == Some(control_text)));
    let repeated = read(&context, &snapshot, None, Some(&refreshed.revision), None).unwrap();
    assert_eq!(repeated.revision, refreshed.revision);
    assert!(repeated.unchanged);
    assert!(!repeated.reset);
    assert!(repeated.events.is_empty());

    let archived = archive.chat_events_for_agent("agent-1").unwrap();
    let archived_ids: BTreeSet<_> = archived
        .iter()
        .filter(|event| event.kind == AgentChatEventKind::Message)
        .map(|event| event.id.as_str())
        .collect();
    assert_eq!(
        archived_ids,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
}

#[test]
fn source_revalidation_follows_live_member_and_restores_owned_rows_after_late_ambiguity() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("source-revalidation.jsonl");
    let text = "source-bound commentary body "
        .repeat(PREVIEW_BYTES / 20 + 32)
        .trim_end()
        .to_owned();
    assert!(text.len() > PREVIEW_BYTES);
    let contents = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        commentary_stream_line_with_text(&text),
        commentary_item_line_with_text("msg-commentary-native", &text)
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    let live_id = first
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("event_msg"))
        .unwrap()
        .id
        .clone();
    let completed_id = first
        .events
        .iter()
        .find(|event| event.source.as_deref() == Some("response_item"))
        .unwrap()
        .id
        .clone();
    archive
        .append_provider_log_batch_with_context(context.clone(), &first.events, None, &first.next)
        .unwrap();
    finish_projection(&archive);
    finish_source_index(&snapshot);

    let qualified = read(&context, &snapshot, None, None, None).unwrap();
    eprintln!(
        "CHAT_REVALIDATION_ACTUAL_PAGE label=H0 requested_ids=null dto={}",
        serde_json::to_string(&qualified).unwrap()
    );
    let indexed = source_index_page(&snapshot);
    assert!(indexed.counts_complete);
    let seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let source_coordinates = |source_id: &str| {
        let event = seed
            .events
            .iter()
            .find(|event| event.id.as_str() == source_id)
            .unwrap();
        (
            event.metadata["chat_source_start"].as_u64().unwrap(),
            event.metadata["chat_source_end"].as_u64().unwrap(),
        )
    };
    let (live_start, live_end) = source_coordinates(&live_id);
    let (completed_seed_start, completed_seed_end) = source_coordinates(&completed_id);
    let indexed_pair = indexed
        .events
        .iter()
        .find(|event| {
            event.metadata["chat_display_physical_id"].as_str() == Some(completed_id.as_str())
        })
        .unwrap();
    assert_eq!(indexed_pair.id, live_id);
    let completed_start = indexed_pair.metadata["chat_source_start"].as_u64().unwrap();
    let completed_end = indexed_pair.metadata["chat_source_end"].as_u64().unwrap();
    assert_eq!(
        (completed_start, completed_end),
        (completed_seed_start, completed_seed_end)
    );

    // The caller knows only the live observation. The backend follows the
    // current indexed relation to B and returns B's physical content under A.
    let live_only = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&qualified.revision),
        None,
        Some(std::slice::from_ref(&live_id)),
    )
    .unwrap();
    eprintln!(
        "CHAT_REVALIDATION_ACTUAL_PAGE label=A-only-current requested_ids={} dto={}",
        serde_json::to_string(std::slice::from_ref(&live_id)).unwrap(),
        serde_json::to_string(&live_only).unwrap()
    );
    assert_eq!(live_only.revalidated_ids, Some(vec![live_id.clone()]));
    assert_eq!(
        serde_json::to_value(&live_only).unwrap()["revalidated_ids"],
        json!([live_id.clone()])
    );
    assert_eq!(
        live_only.generation.as_deref(),
        Some(indexed.generation.as_str())
    );
    assert_eq!(
        live_only.source_epoch.as_deref(),
        Some(indexed.epoch.as_str())
    );
    assert!(!live_only.reset);
    assert_eq!(live_only.progress, qualified.progress);
    assert_eq!(live_only.revision, qualified.revision);
    assert!(live_only.next_before.is_none());
    assert_eq!(live_only.events.len(), 1);
    let live_projection = &live_only.events[0];
    assert_eq!(live_projection.id, live_id);
    assert_eq!(
        live_projection.metadata["chat_display_physical_id"],
        json!(completed_id)
    );
    assert_eq!(
        live_projection.metadata["chat_source_ref"].as_str(),
        Some(completed_id.as_str())
    );
    assert_eq!(live_projection.source.as_deref(), Some("response_item"));
    assert_eq!(
        live_projection.metadata["chat_source_start"],
        json!(completed_start)
    );
    assert_eq!(
        live_projection.metadata["chat_source_end"],
        json!(completed_end)
    );
    let members: BTreeSet<_> = live_projection.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    assert!(live_only
        .aliases
        .iter()
        .any(|alias| { alias.observation_id == completed_id && alias.canonical_id == live_id }));
    assert!(live_only
        .aliases
        .iter()
        .any(|alias| { alias.observation_id == live_id && alias.canonical_id == live_id }));
    assert!(live_only.records_decoded < 32);
    assert!(live_only.bytes_read < 2 * 1024 * 1024);
    let completed_detail = live_projection.metadata["chat_detail_ref"]
        .as_str()
        .expect("the selected physical B row retains its own detail binding");
    let expanded = read_complete_detail_for_event(
        &context,
        &snapshot,
        &live_only.revision,
        completed_detail,
        &live_projection.id,
    );
    assert_eq!(expanded, text);

    let both_members = vec![live_id.clone(), completed_id.clone()];
    let both = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&qualified.revision),
        None,
        Some(both_members.as_slice()),
    )
    .unwrap();
    eprintln!(
        "CHAT_REVALIDATION_ACTUAL_PAGE label=AB-current requested_ids={} dto={}",
        serde_json::to_string(&both_members).unwrap(),
        serde_json::to_string(&both).unwrap()
    );
    assert_eq!(both.revalidated_ids, Some(both_members.clone()));
    assert_eq!(both.events.len(), 1);
    assert_eq!(both.events[0].id, live_id);
    assert_eq!(
        both.events[0].metadata["chat_display_physical_id"],
        json!(completed_id)
    );

    let missing_id = format!(
        "source:agent-1:{}:999999999:{}:0",
        indexed.epoch,
        "c".repeat(64)
    );
    let missing_batch = vec![live_id.clone(), missing_id];
    assert!(super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&qualified.revision),
        None,
        Some(missing_batch.as_slice()),
    )
    .is_err());

    append_native_line(
        &path,
        &commentary_item_line_with_text("msg-commentary-late-duplicate", &text),
    );
    let mut late = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(first.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut late.events, "codex", &path);
    assert!(late
        .events
        .iter()
        .any(|event| event.source.as_deref() == Some("response_item")));
    archive
        .append_provider_log_batch_with_context(
            context.clone(),
            &late.events,
            Some(&first.next),
            &late.next,
        )
        .unwrap();
    finish_projection(&archive);
    finish_source_index(&snapshot);

    let ambiguous = read(&context, &snapshot, None, Some(&qualified.revision), None).unwrap();
    eprintln!(
        "CHAT_REVALIDATION_ACTUAL_PAGE label=H1-after-late-C requested_ids=null dto={}",
        serde_json::to_string(&ambiguous).unwrap()
    );
    assert!(ambiguous.reset);
    let raw_rows = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&ambiguous.revision),
        None,
        Some(both_members.as_slice()),
    )
    .unwrap();
    eprintln!(
        "CHAT_REVALIDATION_ACTUAL_PAGE label=AB-late-C-raw requested_ids={} dto={}",
        serde_json::to_string(&both_members).unwrap(),
        serde_json::to_string(&raw_rows).unwrap()
    );
    assert_eq!(raw_rows.revalidated_ids, Some(both_members));
    assert_eq!(raw_rows.events.len(), 2);
    assert!(raw_rows.next_before.is_none());
    for (event, source_id, source_kind, expected_start, expected_end) in [
        (
            &raw_rows.events[0],
            &live_id,
            "event_msg",
            live_start,
            live_end,
        ),
        (
            &raw_rows.events[1],
            &completed_id,
            "response_item",
            completed_start,
            completed_end,
        ),
    ] {
        assert_eq!(event.id.as_str(), source_id.as_str());
        assert_eq!(
            event.metadata["chat_source_ref"].as_str(),
            Some(source_id.as_str())
        );
        assert_eq!(event.source.as_deref(), Some(source_kind));
        assert_eq!(event.role, Some(AgentChatRole::Assistant));
        let acquired_event = first
            .events
            .iter()
            .find(|acquired| acquired.id.as_str() == source_id.as_str())
            .expect("the original acquisition retains each raw source row");
        assert_eq!(event.turn_id, acquired_event.turn_id);
        if source_kind == "event_msg" {
            assert!(event.turn_id.is_none());
        } else {
            assert_eq!(event.turn_id.as_deref(), Some("msg-commentary-native"));
        }
        assert_eq!(
            event.metadata["provider_turn_id"].as_str(),
            Some("commentary-provider-turn")
        );
        if source_kind == "response_item" {
            assert_ne!(
                event.turn_id.as_deref(),
                event.metadata["provider_turn_id"].as_str(),
                "the raw response-item turn_id remains its native item ID"
            );
        }
        assert_eq!(
            event.metadata["provider_phase"].as_str(),
            Some("commentary")
        );
        assert_eq!(event.metadata["chat_source_start"], json!(expected_start));
        assert_eq!(event.metadata["chat_source_end"], json!(expected_end));
        assert_eq!(
            event.metadata["chat_source_epoch"].as_str(),
            Some(indexed.epoch.as_str())
        );
        assert_eq!(
            event.metadata["chat_source_admission"].as_str(),
            Some(indexed.admission.as_str())
        );
        assert!(event.metadata["chat_display_member_ids"].is_null());
        assert!(event.metadata["chat_detail_ref"].as_str().is_some());
        assert!(raw_rows.aliases.iter().any(|alias| {
            alias.observation_id.as_str() == source_id.as_str()
                && alias.canonical_id.as_str() == source_id.as_str()
        }));
        assert!(!raw_rows.removed_ids.contains(source_id));
    }
    for (event, source_id) in raw_rows
        .events
        .iter()
        .zip([live_id.as_str(), completed_id.as_str()])
    {
        let detail_ref = event.metadata["chat_detail_ref"].as_str().unwrap();
        let expanded = read_complete_detail_for_event(
            &context,
            &snapshot,
            &raw_rows.revision,
            detail_ref,
            source_id,
        );
        assert_eq!(expanded, text);
    }
}

#[test]
fn source_revalidation_retains_loaded_pair_outside_h1_pages_and_restores_raw_rows_after_late_c() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("source-revalidation-older-window.jsonl");
    let text = "source-bound commentary body "
        .repeat(PREVIEW_BYTES / 29 + 32)
        .trim_end()
        .to_owned();
    assert!(text.len() > PREVIEW_BYTES);

    let mut contents = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}})
    );
    for index in 0..162 {
        contents.push_str(&native_tool_call_line(&format!("h0-older-history-{index}")));
        contents.push('\n');
    }
    contents.push_str(&format!(
        "{}\n{}\n{}\n",
        commentary_context_line(),
        commentary_stream_line_with_text(&text),
        commentary_item_line_with_text("msg-commentary-native", &text)
    ));
    for index in 0..78 {
        contents.push_str(&native_tool_call_line(&format!(
            "h0-older-control-after-pair-{index}"
        )));
        contents.push('\n');
    }
    for index in 0..80 {
        contents.push_str(&native_tool_call_line(&format!(
            "h0-recent-history-{index}"
        )));
        contents.push('\n');
    }
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    assert_eq!(
        first
            .events
            .iter()
            .filter(|event| event.kind == AgentChatEventKind::ToolCall)
            .count(),
        320,
        "the preexisting and H0 history are real indexed tool-call rows"
    );
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    let acquired_live = first
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("event_msg")
                && event.kind == AgentChatEventKind::Message
                && event.role == Some(AgentChatRole::Assistant)
                && event.text.as_deref() == Some(text.as_str())
        })
        .unwrap()
        .clone();
    let acquired_completed = first
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("response_item")
                && event.kind == AgentChatEventKind::Message
                && event.role == Some(AgentChatRole::Assistant)
                && event.turn_id.as_deref() == Some("msg-commentary-native")
        })
        .unwrap()
        .clone();
    let live_id = acquired_live.id.clone();
    let completed_id = acquired_completed.id.clone();
    assert_ne!(live_id, completed_id);
    assert_eq!(
        acquired_completed.turn_id.as_deref(),
        Some("msg-commentary-native")
    );
    assert_eq!(
        acquired_completed.metadata["provider_turn_id"].as_str(),
        Some("commentary-provider-turn")
    );

    archive
        .append_provider_log_batch_with_context(context.clone(), &first.events, None, &first.next)
        .unwrap();
    finish_projection(&archive);
    finish_source_index_with_diagnostic(&snapshot, "H0");

    let initial_extent = fs::metadata(&path).unwrap().len();
    eprintln!(
        "CHAT1693_SOURCE_INDEX_BEFORE_PROOF {}",
        source_index_diagnostic_state(&snapshot, None)
    );
    let (epoch, admission, initial_completed_span, initial_tool_call_rows) =
        assert_current_commentary_source_index_proof(
            &snapshot,
            &live_id,
            &completed_id,
            initial_extent,
        );
    assert_eq!(
        initial_tool_call_rows, 320,
        "all 240 preexisting and 80 H0 native ToolCall rows remain indexed"
    );
    let initial_index = source_index_page(&snapshot);
    assert!(initial_index.counts_complete);
    assert_eq!(initial_index.epoch, epoch);
    assert_eq!(initial_index.admission, admission);
    assert_eq!(initial_index.target_extent, initial_extent);
    let initial_generation = initial_index.generation.clone();

    let mut source_cursor = None;
    let mut reached_source_end = false;
    let mut source_spans = std::collections::BTreeMap::new();
    for _ in 0..8 {
        let page = source_index_page_with_cursor(&snapshot, source_cursor.as_deref());
        assert!(page.counts_complete);
        assert_eq!(page.epoch, epoch);
        assert_eq!(page.admission, admission);
        assert_eq!(page.generation, initial_generation);
        assert_eq!(page.target_extent, initial_extent);
        for event in &page.events {
            let source_ref = event.metadata["chat_source_ref"].as_str().unwrap();
            if source_ref == live_id.as_str() || source_ref == completed_id.as_str() {
                let span = (
                    event.metadata["chat_source_start"].as_u64().unwrap(),
                    event.metadata["chat_source_end"].as_u64().unwrap(),
                );
                assert!(source_spans.insert(source_ref.to_owned(), span).is_none());
            }
        }
        source_cursor = page.next_before;
        if source_cursor.is_none() {
            reached_source_end = true;
            break;
        }
    }
    assert!(
        reached_source_end,
        "the bounded fixture fits eight source-index pages"
    );
    assert_eq!(source_spans.len(), 2);
    let live_span = *source_spans.get(&live_id).unwrap();
    let completed_span = *source_spans.get(&completed_id).unwrap();
    assert_eq!(completed_span, initial_completed_span);

    let contains_loaded_pair = |page: &AgentChatPage| {
        page.events.iter().any(|event| {
            event.id == live_id
                || event.id == completed_id
                || event.metadata["chat_display_physical_id"].as_str()
                    == Some(completed_id.as_str())
                || event.metadata["chat_display_member_ids"]
                    .as_array()
                    .is_some_and(|members| {
                        members.iter().any(|member| {
                            matches!(
                                member.as_str(),
                                Some(id)
                                    if id == live_id.as_str() || id == completed_id.as_str()
                            )
                        })
                    })
        })
    };
    let assert_displayed_pair = |page: &AgentChatPage, displayed: &AgentChatEvent| {
        assert_eq!(displayed.id, live_id);
        assert_eq!(displayed.source.as_deref(), Some("response_item"));
        assert_eq!(
            displayed.metadata["chat_display_physical_id"].as_str(),
            Some(completed_id.as_str())
        );
        assert_eq!(
            displayed.metadata["chat_source_ref"].as_str(),
            Some(completed_id.as_str())
        );
        assert_eq!(displayed.metadata["chat_source_epoch"], json!(epoch));
        let members: BTreeSet<_> = displayed.metadata["chat_display_member_ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|member| member.as_str())
            .collect();
        assert_eq!(
            members,
            BTreeSet::from([live_id.as_str(), completed_id.as_str()])
        );
        assert!(displayed
            .text
            .as_deref()
            .is_some_and(|preview| text.starts_with(preview)));
        assert!(page
            .aliases
            .iter()
            .any(|alias| alias.observation_id == completed_id && alias.canonical_id == live_id));
        assert!(!page.removed_ids.contains(&live_id));
    };

    // H0 has 80 recent rows and enough preexisting history for its loaded
    // older window to retain a real continuation frontier past A/B.
    let h0_recent = read(&context, &snapshot, None, None, None).unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "loaded-long-pair-H0-recent",
        &h0_recent,
        &live_id,
        &completed_id,
    );
    trace_revalidation_actual_page("loaded-long-pair-H0-recent-full", None, &h0_recent);
    assert_eq!(h0_recent.events.len(), 80);
    assert!(h0_recent
        .events
        .iter()
        .all(|event| event.kind == AgentChatEventKind::ToolCall));
    assert_eq!(
        h0_recent
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        80,
        "H0's ordinary recent window contains 80 distinct history rows"
    );
    assert!(!contains_loaded_pair(&h0_recent));
    let h0_cursor = h0_recent
        .next_before
        .as_deref()
        .expect("the ordinary H0 recent page exposes its real older cursor");
    assert!(!h0_cursor.is_empty(), "the actual H0 cursor is nonempty");
    let h0_older = read(
        &context,
        &snapshot,
        Some(h0_cursor),
        Some(&h0_recent.revision),
        None,
    )
    .unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "loaded-long-pair-H0-older",
        &h0_older,
        &live_id,
        &completed_id,
    );
    trace_revalidation_actual_page("loaded-long-pair-H0-older-full", None, &h0_older);
    assert!(contains_loaded_pair(&h0_older));
    let h0_older_frontier = h0_older
        .next_before
        .clone()
        .expect("the H0 older page retains a real frontier beyond the loaded pair");
    let h0_pair = h0_older
        .events
        .iter()
        .find(|event| event.id == live_id)
        .expect("the ordinary H0 older page loads public A");
    // An ordinary canonical display page may omit the source-index admission
    // annotation. Successful revalidation responses are checked for it below.
    assert_displayed_pair(&h0_older, h0_pair);
    let h0_loaded_pages = [&h0_older, &h0_recent];
    let h0_loaded_source_ids =
        loaded_source_ids_from_pages(&context.agent_id, &live_id, &h0_loaded_pages);
    assert!(h0_loaded_source_ids.len() > super::MAX_REVALIDATED_SOURCE_IDS);
    assert!(h0_loaded_source_ids.contains(&live_id));
    assert!(h0_loaded_source_ids.contains(&completed_id));
    assert_loaded_source_ids_start_with_oldest_row(
        &context.agent_id,
        &h0_loaded_pages,
        &h0_loaded_source_ids,
    );
    assert_eq!(h0_loaded_source_ids.len(), 160);
    let h0_older_batches = source_id_batches(&h0_loaded_source_ids);
    assert_eq!(h0_older_batches.len(), 20);
    trace_revalidation_batch_plan(
        "loaded-window-H0-older",
        &h0_loaded_source_ids,
        &h0_older_batches,
    );
    let mut h0_revalidated_ids = BTreeSet::new();
    for (batch_index, batch) in h0_older_batches.iter().enumerate() {
        let response = super::read_with_revalidation(
            &context,
            &snapshot,
            None,
            Some(&h0_older.revision),
            None,
            Some(batch.as_slice()),
        )
        .unwrap();
        trace_revalidation_actual_page(
            &format!("loaded-window-H0-older-batch-{batch_index}"),
            Some(batch),
            &response,
        );
        assert_eq!(response.revalidated_ids.as_deref(), Some(batch.as_slice()));
        assert_eq!(response.revision, h0_older.revision);
        assert_eq!(response.session_id, h0_older.session_id);
        assert_eq!(response.conversation_id, h0_older.conversation_id);
        assert_eq!(response.generation, h0_older.generation);
        assert_eq!(response.source_epoch, h0_older.source_epoch);
        assert_revalidated_page_covers_ids(
            &response,
            batch,
            &initial_generation,
            &epoch,
            &admission,
        );
        assert!(
            response.next_before.is_none(),
            "H0 renewal does not advance the ordinary older cursor"
        );
        assert!(
            response.bytes_read <= super::super::chat_read_store::READ_BYTES,
            "each H0 renewal stays within the existing one-MiB read budget"
        );
        assert!(
            response.records_decoded <= super::super::chat_read_store::READ_OBJECTS,
            "each H0 renewal stays within the existing 512-object read budget"
        );
        for source_id in batch {
            assert!(h0_revalidated_ids.insert(source_id.clone()));
        }
    }
    assert_eq!(
        h0_revalidated_ids,
        h0_loaded_source_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
    );

    // Append exactly one recent page. These ordinary source rows remain in the
    // archive and page normally without adding unrelated message identities.
    for index in 0..80 {
        append_native_line(
            &path,
            &native_tool_call_line(&format!("h1-recent-history-{index}")),
        );
    }
    // Advance ordinary archive ingestion too; source-index completion alone leaves canonical older cursors at H0.
    let mut h1_capture = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(first.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut h1_capture.events,
        "codex",
        &path,
    );
    assert_eq!(
        h1_capture.events.len(),
        80,
        "ordinary H1 capture ingests all 80 appended history rows"
    );
    assert!(h1_capture
        .events
        .iter()
        .all(|event| event.kind == AgentChatEventKind::ToolCall));
    archive
        .append_provider_log_batch_with_context(
            context.clone(),
            &h1_capture.events,
            Some(&first.next),
            &h1_capture.next,
        )
        .unwrap();
    finish_projection(&archive);
    finish_source_index_with_diagnostic(&snapshot, "H1");
    let h1_extent = fs::metadata(&path).unwrap().len();
    let (h1_epoch, h1_admission, h1_completed_span, h1_tool_call_rows) =
        assert_current_commentary_source_index_proof(&snapshot, &live_id, &completed_id, h1_extent);
    assert_eq!(
        h1_tool_call_rows, 400,
        "the H1 source walk retains all 320 prior and 80 appended ToolCall rows"
    );
    assert_eq!(h1_epoch, epoch);
    assert_eq!(h1_admission, admission);
    assert_eq!(h1_completed_span, completed_span);
    let h1_index = source_index_page(&snapshot);
    assert!(h1_index.counts_complete);
    assert_eq!(h1_index.epoch, epoch);
    assert_eq!(h1_index.admission, admission);
    assert_eq!(h1_index.target_extent, h1_extent);
    assert_eq!(h1_index.generation, initial_generation);

    let h1_recent = read(&context, &snapshot, None, Some(&h0_older.revision), None).unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "loaded-pair-H1-recent",
        &h1_recent,
        &live_id,
        &completed_id,
    );
    trace_revalidation_actual_page("loaded-pair-H1-recent-full", None, &h1_recent);
    assert!(h1_recent.reset);
    assert_eq!(h1_recent.session_id, h0_recent.session_id);
    assert_eq!(h1_recent.conversation_id, h0_recent.conversation_id);
    assert_eq!(h1_recent.generation, h0_recent.generation);
    assert_eq!(h1_recent.source_epoch, h0_recent.source_epoch);
    assert_eq!(h1_recent.events.len(), 80);
    assert!(h1_recent
        .events
        .iter()
        .all(|event| event.kind == AgentChatEventKind::ToolCall));
    assert_eq!(
        h1_recent
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        80,
        "the appended H1 recent window contains 80 distinct history rows"
    );
    assert!(!contains_loaded_pair(&h1_recent));
    eprintln!(
        "CHAT_REVALIDATION_OLDER_REQUEST label=H1 cursor={}",
        serde_json::to_string(&h0_older_frontier).unwrap()
    );
    let h1_older = read(&context, &snapshot, Some(&h0_older_frontier), None, None).unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "loaded-pair-H1-first-older",
        &h1_older,
        &live_id,
        &completed_id,
    );
    trace_revalidation_actual_page("loaded-pair-H1-first-older-full", None, &h1_older);
    assert_eq!(h1_older.conversation_id, h0_recent.conversation_id);
    assert_eq!(h1_older.generation, h0_recent.generation);
    assert_eq!(h1_older.source_epoch, h0_recent.source_epoch);
    assert_eq!(h1_older.events.len(), 80);
    assert!(h1_older
        .events
        .iter()
        .all(|event| event.kind == AgentChatEventKind::ToolCall));
    assert!(!contains_loaded_pair(&h1_older));
    let h1_older_frontier = h1_older
        .next_before
        .clone()
        .expect("the H1 older response preserves a real frontier for H2");
    assert!(!h1_older_frontier.is_empty());
    let h1_current_revision = h1_recent.revision.clone();
    // Applying the actual older page prepends it to the already loaded window.
    let h1_loaded_pages = [&h1_older, &h0_older, &h0_recent];
    let h1_loaded_source_ids =
        loaded_source_ids_from_pages(&context.agent_id, &live_id, &h1_loaded_pages);
    let h0_loaded_set: BTreeSet<_> = h0_loaded_source_ids.iter().cloned().collect();
    let h1_loaded_set: BTreeSet<_> = h1_loaded_source_ids.iter().cloned().collect();
    assert!(h0_loaded_set.is_subset(&h1_loaded_set));
    assert!(h1_loaded_source_ids.len() > super::MAX_REVALIDATED_SOURCE_IDS);
    assert!(h1_loaded_set.contains(&live_id));
    assert!(h1_loaded_set.contains(&completed_id));
    assert_loaded_source_ids_start_with_oldest_row(
        &context.agent_id,
        &h1_loaded_pages,
        &h1_loaded_source_ids,
    );
    let h1_batches = source_id_batches(&h1_loaded_source_ids);
    trace_revalidation_batch_plan("loaded-window-H1-older", &h1_loaded_source_ids, &h1_batches);
    assert!(h1_batches.len() > 1);
    assert_eq!(h1_loaded_source_ids.len(), 240);
    assert_eq!(h1_batches.len(), 30);
    let mut h1_older_revalidated_ids = BTreeSet::new();
    for (batch_index, batch) in h1_batches.iter().enumerate() {
        let response = super::read_with_revalidation(
            &context,
            &snapshot,
            None,
            Some(&h1_older.revision),
            None,
            Some(batch.as_slice()),
        )
        .unwrap();
        trace_revalidation_actual_page(
            &format!("loaded-window-H1-older-batch-{batch_index}"),
            Some(batch),
            &response,
        );
        assert_eq!(response.revalidated_ids.as_deref(), Some(batch.as_slice()));
        assert_eq!(response.revision, h1_older.revision);
        assert_eq!(response.session_id, h1_older.session_id);
        assert_eq!(response.conversation_id, h1_older.conversation_id);
        assert_eq!(response.generation, h1_older.generation);
        assert_eq!(response.source_epoch, h1_older.source_epoch);
        assert_revalidated_page_covers_ids(
            &response,
            batch,
            &initial_generation,
            &epoch,
            &admission,
        );
        assert!(
            response.next_before.is_none(),
            "H1 renewal does not advance the ordinary older cursor"
        );
        assert!(
            response.bytes_read <= super::super::chat_read_store::READ_BYTES,
            "each H1 older-token renewal stays within the existing one-MiB read budget"
        );
        assert!(
            response.records_decoded <= super::super::chat_read_store::READ_OBJECTS,
            "each H1 older-token renewal stays within the existing 512-object read budget"
        );
        for source_id in batch {
            assert!(h1_older_revalidated_ids.insert(source_id.clone()));
        }
    }
    assert_eq!(h1_older_revalidated_ids, h1_loaded_set);

    let a_only_ids = vec![live_id.clone()];
    let a_only = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&h1_current_revision),
        None,
        Some(a_only_ids.as_slice()),
    )
    .unwrap();
    trace_revalidation_actual_page("loaded-pair-H1-A-only", Some(&a_only_ids), &a_only);
    assert_eq!(a_only.revalidated_ids, Some(a_only_ids.clone()));
    assert_eq!(a_only.revision, h1_current_revision);
    assert_eq!(a_only.events.len(), 1);
    assert_revalidated_page_covers_ids(
        &a_only,
        &a_only_ids,
        &initial_generation,
        &epoch,
        &admission,
    );
    assert_displayed_pair(&a_only, &a_only.events[0]);

    // A recent metadata poll changes no membership; it renews this same loaded window.
    let h1_recent_loaded_source_ids =
        loaded_source_ids_from_pages(&context.agent_id, &live_id, &h1_loaded_pages);
    assert_eq!(h1_recent_loaded_source_ids, h1_loaded_source_ids);
    assert_eq!(h1_recent_loaded_source_ids.len(), 240);
    let h1_recent_loaded_set: BTreeSet<_> = h1_recent_loaded_source_ids.iter().cloned().collect();
    assert_loaded_source_ids_start_with_oldest_row(
        &context.agent_id,
        &h1_loaded_pages,
        &h1_recent_loaded_source_ids,
    );
    let h1_recent_batches = source_id_batches(&h1_recent_loaded_source_ids);
    assert_eq!(h1_recent_batches.len(), 30);
    trace_revalidation_batch_plan(
        "loaded-window-H1-recent",
        &h1_recent_loaded_source_ids,
        &h1_recent_batches,
    );
    let mut h1_revalidated_ids = BTreeSet::new();
    let mut renewed_pair = false;
    for (batch_index, batch) in h1_recent_batches.iter().enumerate() {
        let response = super::read_with_revalidation(
            &context,
            &snapshot,
            None,
            Some(&h1_current_revision),
            None,
            Some(batch.as_slice()),
        )
        .unwrap();
        trace_revalidation_actual_page(
            &format!("loaded-window-H1-recent-batch-{batch_index}"),
            Some(batch),
            &response,
        );
        assert_eq!(response.revalidated_ids.as_deref(), Some(batch.as_slice()));
        assert_eq!(response.revision, h1_current_revision);
        assert_eq!(response.session_id, h1_older.session_id);
        assert_eq!(response.conversation_id, h1_older.conversation_id);
        assert_eq!(response.generation, h1_older.generation);
        assert_eq!(response.source_epoch, h1_older.source_epoch);
        assert_revalidated_page_covers_ids(
            &response,
            batch,
            &initial_generation,
            &epoch,
            &admission,
        );
        for source_id in batch {
            assert!(h1_revalidated_ids.insert(source_id.clone()));
        }
        if batch.contains(&live_id) || batch.contains(&completed_id) {
            assert!(batch.contains(&live_id) && batch.contains(&completed_id));
            let displayed_pair = response
                .events
                .iter()
                .find(|event| event.id == live_id)
                .expect("the actual H1 cohort batch renews the inspected pair");
            assert_displayed_pair(&response, displayed_pair);
            assert!(!renewed_pair);
            renewed_pair = true;
        }
    }
    assert!(renewed_pair, "the H1 recent cohort renews the loaded pair");
    assert_eq!(h1_revalidated_ids, h1_recent_loaded_set);

    let h1_pair_page = source_index_page_with_cursor(&snapshot, Some(h0_cursor));
    assert!(h1_pair_page.counts_complete);
    let persisted_pair = h1_pair_page
        .events
        .iter()
        .find(|event| {
            event.metadata["chat_source_ref"].as_str() == Some(completed_id.as_str())
                && event.metadata["chat_display_member_ids"]
                    .as_array()
                    .is_some_and(|members| {
                        members
                            .iter()
                            .filter_map(|member| member.as_str())
                            .collect::<BTreeSet<_>>()
                            == BTreeSet::from([live_id.as_str(), completed_id.as_str()])
                    })
        })
        .expect("the completed H1 native head stores the qualified A/B relation");
    assert_eq!(persisted_pair.id, live_id);
    assert_eq!(
        persisted_pair.metadata["chat_display_physical_id"].as_str(),
        Some(completed_id.as_str())
    );
    assert_eq!(
        persisted_pair.metadata["chat_source_ref"].as_str(),
        Some(completed_id.as_str())
    );
    let positive_ordinary_pair_page =
        read(&context, &snapshot, Some(h0_cursor), None, None).unwrap();
    let positive_ordinary_pair = positive_ordinary_pair_page
        .events
        .iter()
        .find(|event| {
            event.id == live_id
                && event.metadata["chat_source_ref"].as_str() == Some(completed_id.as_str())
                && event.metadata["chat_display_physical_id"].as_str()
                    == Some(completed_id.as_str())
        })
        .expect("the ordinary older page publishes the current public A / physical B pair");
    assert_eq!(positive_ordinary_pair.id, live_id);
    assert_eq!(
        positive_ordinary_pair.metadata["chat_display_member_ids"],
        json!([live_id, completed_id])
    );
    assert!(positive_ordinary_pair.metadata["chat_body_binding"]
        .as_str()
        .is_some());
    assert_eq!(positive_ordinary_pair.metadata["chat_body_pending"], false);
    let positive_detail_ref = positive_ordinary_pair.metadata["chat_detail_ref"]
        .as_str()
        .expect("the current pair keeps its body detail reference");
    assert_eq!(
        read_complete_detail_for_event(
            &context,
            &snapshot,
            &positive_ordinary_pair_page.revision,
            positive_detail_ref,
            &live_id,
        ),
        text,
    );
    let h1_source_head = super::super::chat_source_index::pointer("agent-1")
        .unwrap()
        .unwrap();
    for index in 0..80 {
        append_native_line(
            &path,
            &native_tool_call_line(&format!("h2-unindexed-history-{index}")),
        );
    }
    let mut h2_history_capture =
        crate::commands::provider_log_acquisition::acquire_provider_log_batch(
            "agent-1",
            "codex",
            &path,
            context.provider_source_key.as_deref().unwrap(),
            Some(h1_capture.next.clone()),
            true,
        )
        .unwrap();
    assert_eq!(h2_history_capture.events.len(), 80);
    assert!(h2_history_capture
        .events
        .iter()
        .all(|event| event.kind == AgentChatEventKind::ToolCall));
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut h2_history_capture.events,
        "codex",
        &path,
    );
    archive
        .append_provider_log_batch_with_context(
            context.clone(),
            &h2_history_capture.events,
            Some(&h1_capture.next),
            &h2_history_capture.next,
        )
        .unwrap();
    finish_projection(&archive);
    append_native_line(
        &path,
        &commentary_item_line_with_text("msg-commentary-late-C", &text),
    );
    let mut h2_capture = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(h2_history_capture.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut h2_capture.events,
        "codex",
        &path,
    );
    assert_eq!(h2_capture.events.len(), 1);
    assert_eq!(
        h2_capture.events[0].source.as_deref(),
        Some("response_item")
    );
    archive
        .append_provider_log_batch_with_context(
            context.clone(),
            &h2_capture.events,
            Some(&h2_history_capture.next),
            &h2_capture.next,
        )
        .unwrap();
    finish_projection(&archive);
    let h2_extent_before_index = fs::metadata(&path).unwrap().len();
    let lagging_source_page = source_index_page_with_cursor(&snapshot, Some(h0_cursor));
    let lagging_source_head = super::super::chat_source_index::pointer("agent-1")
        .unwrap()
        .unwrap();
    assert_eq!(lagging_source_head, h1_source_head);
    assert!(!lagging_source_page.counts_complete);
    assert_eq!(lagging_source_page.epoch, epoch);
    assert_eq!(lagging_source_page.admission, admission);
    assert!(lagging_source_page.target_extent < h2_extent_before_index);
    assert_eq!(lagging_source_page.target_extent, h1_extent);
    assert_eq!(lagging_source_page.events.len(), 80);
    let lagging_source_refs: BTreeSet<_> = lagging_source_page
        .events
        .iter()
        .filter_map(|event| event.metadata["chat_source_ref"].as_str())
        .collect();
    assert!(lagging_source_refs.contains(live_id.as_str()));
    assert!(lagging_source_refs.contains(completed_id.as_str()));
    assert_eq!(lagging_source_refs.len(), 80);
    let pair_source_refs: BTreeSet<_> = lagging_source_page
        .events
        .iter()
        .filter(|event| event.kind != AgentChatEventKind::ToolCall)
        .filter_map(|event| event.metadata["chat_source_ref"].as_str())
        .collect();
    assert_eq!(
        pair_source_refs,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    let pair_end = live_span.1.max(completed_span.1);
    let unrelated_older_rows = lagging_source_page
        .events
        .iter()
        .filter(|event| event.kind == AgentChatEventKind::ToolCall)
        .collect::<Vec<_>>();
    assert_eq!(unrelated_older_rows.len(), 78);
    assert!(unrelated_older_rows.iter().all(|event| {
        event.metadata["chat_source_start"]
            .as_u64()
            .is_some_and(|start| start >= pair_end)
    }));

    let canonical_ahead = read(&context, &snapshot, Some(h0_cursor), None, None).unwrap();
    eprintln!(
        "CHAT1693_CANONICAL_AHEAD_DIAG {}",
        serde_json::json!({
            "progress": &canonical_ahead.progress,
            "reset": canonical_ahead.reset,
            "events_len": canonical_ahead.events.len(),
            "records_decoded": canonical_ahead.records_decoded,
            "bytes_read": canonical_ahead.bytes_read,
            "next_before": &canonical_ahead.next_before,
            "incoming_cursor": h0_cursor,
            "pair_events": canonical_ahead.events.iter().filter(|event| {
                event.metadata["chat_source_ref"].as_str().is_some_and(|source_ref| {
                    source_ref == live_id.as_str() || source_ref == completed_id.as_str()
                })
            }).map(|event| serde_json::json!({
                "id": &event.id,
                "source_ref": event.metadata["chat_source_ref"].as_str(),
                "members": event.metadata.get("chat_display_member_ids"),
                "physical_id": event.metadata["chat_display_physical_id"].as_str(),
                "removed_metadata": event.metadata.get("chat_display_removed_ids"),
                "source_start": event.metadata["chat_source_start"].as_u64(),
                "source_end": event.metadata["chat_source_end"].as_u64(),
                "detail_ref_present": event.metadata["chat_detail_ref"].is_string(),
                "body_binding_present": event.metadata["chat_body_binding"].is_string(),
            })).collect::<Vec<_>>(),
            "pair_aliases": canonical_ahead.aliases.iter().filter(|alias| {
                [live_id.as_str(), completed_id.as_str()].contains(&alias.observation_id.as_str())
                    || [live_id.as_str(), completed_id.as_str()].contains(&alias.canonical_id.as_str())
            }).map(|alias| serde_json::json!({
                "observation_id": &alias.observation_id,
                "canonical_id": &alias.canonical_id,
            })).collect::<Vec<_>>(),
            "pair_removed_ids": canonical_ahead.removed_ids.iter().filter(|id| {
                [live_id.as_str(), completed_id.as_str()].contains(&id.as_str())
            }).collect::<Vec<_>>(),
        })
    );
    assert!(!canonical_ahead.removed_ids.contains(&live_id));
    assert!(!canonical_ahead.removed_ids.contains(&completed_id));
    assert_eq!(canonical_ahead.events.len(), 80);
    for (source_id, captured) in [
        (&live_id, &acquired_live),
        (&completed_id, &acquired_completed),
    ] {
        let raw = canonical_ahead
            .events
            .iter()
            .find(|event| event.metadata["chat_source_ref"].as_str() == Some(source_id.as_str()))
            .expect("canonical retraction restores each loaded member as its own source row");
        assert_eq!(raw.id, *source_id);
        assert!(canonical_ahead.aliases.iter().any(|alias| {
            alias.observation_id.as_str() == source_id.as_str()
                && alias.canonical_id.as_str() == source_id.as_str()
        }));
        assert!(raw.metadata["chat_display_member_ids"].is_null());
        assert!(raw.metadata["chat_body_binding"].as_str().is_some());
        assert_eq!(raw.metadata["chat_body_pending"], false);
        assert_eq!(
            raw.metadata["chat_source_start"],
            captured.metadata["chat_source_start"]
        );
        assert_eq!(
            raw.metadata["chat_source_end"],
            captured.metadata["chat_source_end"]
        );
        let detail_ref = raw.metadata["chat_detail_ref"]
            .as_str()
            .expect("each restored native row keeps its own complete-body reference");
        assert_eq!(
            read_complete_detail_for_event(
                &context,
                &snapshot,
                &canonical_ahead.revision,
                detail_ref,
                source_id,
            ),
            text,
        );
    }
    assert!(!canonical_ahead.aliases.iter().any(|alias| {
        (alias.observation_id == live_id && alias.canonical_id == completed_id)
            || (alias.observation_id == completed_id && alias.canonical_id == live_id)
    }));
    finish_source_index_with_diagnostic(&snapshot, "H2");
    let h2_extent = fs::metadata(&path).unwrap().len();
    let h2_index = source_index_page(&snapshot);
    assert!(h2_index.counts_complete);
    assert_eq!(h2_index.epoch, epoch);
    assert_eq!(h2_index.admission, admission);
    assert_eq!(h2_index.target_extent, h2_extent);
    assert_eq!(h2_index.generation, initial_generation);
    let h2_seed = crate::commands::chat_recent_seed::read(&snapshot, None).unwrap();
    let captured_late_c = &h2_capture.events[0];
    assert_eq!(captured_late_c.kind, AgentChatEventKind::Message);
    assert_eq!(captured_late_c.role, Some(AgentChatRole::Assistant));
    assert_eq!(
        captured_late_c.turn_id.as_deref(),
        Some("msg-commentary-late-C")
    );
    assert_eq!(
        captured_late_c.metadata["provider_phase"],
        json!("commentary")
    );
    assert_eq!(captured_late_c.metadata["chat_source_epoch"], json!(epoch));
    assert_eq!(captured_late_c.text.as_deref(), Some(text.as_str()));
    let late_c_id = captured_late_c.metadata["chat_source_ref"]
        .as_str()
        .expect("the acquired late commentary row has its native source reference")
        .to_owned();
    assert_eq!(
        captured_late_c.id, late_c_id,
        "the captured source ID is the row identity, not its native turn ID"
    );

    let late_c_seed_candidates = h2_seed
        .events
        .iter()
        .filter(|event| {
            event.metadata["chat_source_ref"].as_str() == Some(late_c_id.as_str())
                || event.turn_id.as_deref() == captured_late_c.turn_id.as_deref()
        })
        .map(|event| {
            json!({
                "id": event.id,
                "source_ref": event.metadata["chat_source_ref"],
                "source": event.source,
                "kind": format!("{:?}", event.kind),
                "role": format!("{:?}", event.role),
                "turn_id": event.turn_id,
                "provider_phase": event.metadata["provider_phase"],
                "source_epoch": event.metadata["chat_source_epoch"],
                "source_admission": event.metadata["chat_source_admission"],
                "text_preview_bytes": event.text.as_ref().map(String::len),
                "detail_ref_present": event.metadata["chat_detail_ref"].is_string()
            })
        })
        .collect::<Vec<_>>();
    eprintln!(
        "CHAT1693_LATE_C_SEED_DIAGNOSTIC {}",
        json!({
            "captured_source_ref": late_c_id,
            "captured_full_text_bytes": captured_late_c.text.as_ref().map(String::len),
            "seed_progress": h2_seed.progress,
            "seed_event_count": h2_seed.events.len(),
            "seed_checkpoint": h2_seed.checkpoint.as_ref().map(|checkpoint| {
                json!({
                    "extent": checkpoint.extent,
                    "before": checkpoint.before,
                    "window_complete": checkpoint.window_complete,
                    "normalizer_complete": checkpoint.normalizer_complete
                })
            }),
            "candidate_headers": late_c_seed_candidates
        })
    );
    let late_c_headers = h2_seed
        .events
        .iter()
        .filter(|event| event.metadata["chat_source_ref"].as_str() == Some(late_c_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        late_c_headers.len(),
        1,
        "the current H2 seed has exactly one header for captured C's source row"
    );
    let late_c_header = late_c_headers[0];
    assert_eq!(late_c_header.id, late_c_id);
    assert_eq!(late_c_header.source.as_deref(), Some("response_item"));
    assert_eq!(late_c_header.kind, AgentChatEventKind::Message);
    assert_eq!(late_c_header.role, Some(AgentChatRole::Assistant));
    assert_eq!(
        late_c_header.turn_id.as_deref(),
        Some("msg-commentary-late-C")
    );
    assert_eq!(
        late_c_header.metadata["provider_phase"],
        json!("commentary")
    );
    assert_eq!(late_c_header.metadata["chat_source_ref"], json!(late_c_id));
    assert_eq!(late_c_header.metadata["chat_source_epoch"], json!(epoch));
    assert_eq!(
        late_c_header.metadata["chat_source_admission"],
        json!(admission)
    );
    let late_c_preview = late_c_header
        .text
        .as_deref()
        .expect("the H2 source header retains C's bounded preview");
    assert!(text.starts_with(late_c_preview));
    assert!(late_c_preview.len() <= PREVIEW_BYTES);
    let late_c_detail_ref = late_c_header.metadata["chat_detail_ref"]
        .as_str()
        .expect("the H2 header retains C's own full-body reference");
    let late_c_detail =
        crate::commands::chat_recent_seed::detail(&snapshot, late_c_detail_ref).unwrap();
    assert_eq!(late_c_detail.event_id, late_c_id);
    assert_eq!(late_c_detail.text.as_bytes(), text.as_bytes());
    assert!(late_c_detail.complete);
    assert_ne!(late_c_id, live_id);
    assert_ne!(late_c_id, completed_id);

    let mut h2_source_cursor = None;
    let mut h2_source_refs = BTreeSet::new();
    let mut h2_reached_source_end = false;
    for _ in 0..8 {
        let page = source_index_page_with_cursor(&snapshot, h2_source_cursor.as_deref());
        assert!(page.counts_complete);
        assert_eq!(page.epoch, epoch);
        assert_eq!(page.admission, admission);
        assert_eq!(page.generation, initial_generation);
        assert_eq!(page.target_extent, h2_extent);
        for event in &page.events {
            let source_ref = event.metadata["chat_source_ref"].as_str().unwrap();
            if source_ref == live_id.as_str()
                || source_ref == completed_id.as_str()
                || source_ref == late_c_id.as_str()
            {
                assert_eq!(event.id.as_str(), source_ref);
                assert!(event.metadata["chat_display_member_ids"].is_null());
                assert!(event.metadata["chat_display_physical_id"].is_null());
                assert_eq!(event.metadata["chat_source_epoch"], json!(epoch));
                assert_eq!(event.metadata["chat_source_admission"], json!(admission));
                h2_source_refs.insert(source_ref.to_owned());
            }
        }
        h2_source_cursor = page.next_before;
        if h2_source_cursor.is_none() {
            h2_reached_source_end = true;
            break;
        }
    }
    assert!(h2_reached_source_end);
    assert_eq!(
        h2_source_refs,
        BTreeSet::from([live_id.clone(), completed_id.clone(), late_c_id.clone(),])
    );

    let h2_recent = read(&context, &snapshot, None, Some(&h1_recent.revision), None).unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "loaded-pair-H2-after-late-C-recent",
        &h2_recent,
        &live_id,
        &completed_id,
    );
    trace_revalidation_actual_page("loaded-pair-H2-after-late-C-recent-full", None, &h2_recent);
    assert_eq!(fs::metadata(&path).unwrap().len(), h2_extent);
    // The late-C poll changes the opaque revision while keeping the loaded window scope.
    // Renewal uses that actual revision even when reset is false; the H2 recent page
    // does not add C to the already-loaded older cohort.
    assert_ne!(h2_recent.revision, h1_recent.revision);
    assert_eq!(h2_recent.conversation_id, h0_recent.conversation_id);
    assert_eq!(h2_recent.generation, h0_recent.generation);
    assert_eq!(h2_recent.source_epoch, h0_recent.source_epoch);
    assert!(!contains_loaded_pair(&h2_recent));
    assert!(h2_recent.events.iter().any(|event| event.id == late_c_id));
    eprintln!(
        "CHAT_REVALIDATION_OLDER_REQUEST label=H2 cursor={}",
        serde_json::to_string(&h1_older_frontier).unwrap()
    );
    let h2_older = read(&context, &snapshot, Some(&h1_older_frontier), None, None).unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "loaded-pair-H2-first-older-after-late-C",
        &h2_older,
        &live_id,
        &completed_id,
    );
    trace_revalidation_actual_page(
        "loaded-pair-H2-first-older-after-late-C-full",
        None,
        &h2_older,
    );
    assert_eq!(h2_older.conversation_id, h0_recent.conversation_id);
    assert_eq!(h2_older.generation, h0_recent.generation);
    assert_eq!(h2_older.source_epoch, h0_recent.source_epoch);
    assert!(!contains_loaded_pair(&h2_older));
    assert_eq!(h2_older.events.len(), 80);
    assert!(h2_older
        .events
        .iter()
        .all(|event| event.kind == AgentChatEventKind::ToolCall));
    assert!(h2_older.next_before.is_some());

    let h2_current_revision = h2_recent.revision.clone();
    let h2_loaded_pages = [&h2_older, &h1_older, &h0_older, &h0_recent];
    let h2_loaded_source_ids =
        loaded_source_ids_from_pages(&context.agent_id, &live_id, &h2_loaded_pages);
    let h2_loaded_set: BTreeSet<_> = h2_loaded_source_ids.iter().cloned().collect();
    assert!(h1_loaded_set.is_subset(&h2_loaded_set));
    assert!(
        !h2_loaded_set.contains(&late_c_id),
        "the H2 recent metadata poll does not add C to the older-loaded window"
    );
    assert!(h2_loaded_source_ids.len() > super::MAX_REVALIDATED_SOURCE_IDS);
    assert_eq!(h2_loaded_source_ids.len(), 320);
    assert!(h2_loaded_set.contains(&live_id));
    assert!(h2_loaded_set.contains(&completed_id));
    assert_loaded_source_ids_start_with_oldest_row(
        &context.agent_id,
        &h2_loaded_pages,
        &h2_loaded_source_ids,
    );
    let h2_batches = source_id_batches(&h2_loaded_source_ids);
    trace_revalidation_batch_plan(
        "loaded-window-H2-recent-after-late-C",
        &h2_loaded_source_ids,
        &h2_batches,
    );
    assert!(h2_batches.len() > 1);
    assert_eq!(h2_batches.len(), 40);
    assert!(h2_batches
        .iter()
        .any(|batch| batch.contains(&live_id) && batch.contains(&completed_id)));

    let mut h2_revalidated_ids = BTreeSet::new();
    let mut raw_pair_page = None;
    for (batch_index, batch) in h2_batches.iter().enumerate() {
        let response = super::read_with_revalidation(
            &context,
            &snapshot,
            None,
            Some(&h2_current_revision),
            None,
            Some(batch.as_slice()),
        )
        .unwrap();
        trace_revalidation_actual_page(
            &format!("loaded-window-H2-recent-after-late-C-batch-{batch_index}"),
            Some(batch),
            &response,
        );
        assert_eq!(response.revalidated_ids.as_deref(), Some(batch.as_slice()));
        assert_eq!(response.revision, h2_current_revision);
        assert_eq!(response.session_id, h2_older.session_id);
        assert_eq!(response.conversation_id, h2_older.conversation_id);
        assert_eq!(response.generation, h2_older.generation);
        assert_eq!(response.source_epoch, h2_older.source_epoch);
        assert_revalidated_page_covers_ids(
            &response,
            batch,
            &initial_generation,
            &epoch,
            &admission,
        );
        for source_id in batch {
            assert!(h2_revalidated_ids.insert(source_id.clone()));
        }
        if batch.contains(&live_id) && batch.contains(&completed_id) {
            assert!(raw_pair_page.is_none());
            raw_pair_page = Some((batch.clone(), response));
        }
    }
    assert_eq!(h2_revalidated_ids, h2_loaded_set);
    let (raw_pair_batch, raw_rows) =
        raw_pair_page.expect("an H2 batch renews both loaded pair members");
    assert_eq!(raw_rows.revalidated_ids, Some(raw_pair_batch.clone()));
    assert_revalidated_page_covers_ids(
        &raw_rows,
        &raw_pair_batch,
        &initial_generation,
        &epoch,
        &admission,
    );
    assert!(raw_rows.next_before.is_none());
    assert_eq!(raw_rows.events.len(), raw_pair_batch.len());
    assert!(raw_rows.bytes_read < 2 * 1024 * 1024);
    // Keyed row/member lookups share the reader's configured object budget.
    assert!(raw_rows.records_decoded <= super::super::chat_read_store::READ_OBJECTS);
    assert_eq!(
        raw_rows
            .events
            .iter()
            .filter(|event| event.id == live_id)
            .count(),
        1
    );
    assert_eq!(
        raw_rows
            .events
            .iter()
            .filter(|event| event.id == completed_id)
            .count(),
        1
    );
    let raw_pair_events = [
        raw_rows
            .events
            .iter()
            .find(|event| event.id == live_id)
            .expect("late-C repair returns A's own native observation"),
        raw_rows
            .events
            .iter()
            .find(|event| event.id == completed_id)
            .expect("late-C repair returns B's own native observation"),
    ];
    for (event, (source_id, source_kind, span, acquired)) in raw_pair_events.into_iter().zip([
        (&live_id, "event_msg", live_span, &acquired_live),
        (
            &completed_id,
            "response_item",
            completed_span,
            &acquired_completed,
        ),
    ]) {
        assert_eq!(event.id.as_str(), source_id.as_str());
        assert_eq!(event.turn_id, acquired.turn_id);
        assert_eq!(event.source.as_deref(), Some(source_kind));
        assert_eq!(event.role, Some(AgentChatRole::Assistant));
        assert_eq!(
            event.metadata["chat_source_ref"].as_str(),
            Some(source_id.as_str())
        );
        assert_eq!(event.metadata["chat_source_start"].as_u64(), Some(span.0));
        assert_eq!(event.metadata["chat_source_end"].as_u64(), Some(span.1));
        assert_eq!(
            event.metadata["chat_source_epoch"].as_str(),
            Some(epoch.as_str())
        );
        assert_eq!(
            event.metadata["chat_source_admission"].as_str(),
            Some(admission.as_str())
        );
        assert_eq!(
            event.metadata["provider_turn_id"].as_str(),
            Some("commentary-provider-turn")
        );
        if source_kind == "response_item" {
            assert_eq!(event.turn_id.as_deref(), Some("msg-commentary-native"));
            assert_ne!(
                event.turn_id.as_deref(),
                event.metadata["provider_turn_id"].as_str()
            );
        }
        assert!(event.metadata["chat_display_member_ids"].is_null());
        assert!(event.metadata["chat_display_physical_id"].is_null());
        assert!(raw_rows.aliases.iter().any(|alias| {
            alias.observation_id.as_str() == source_id.as_str()
                && alias.canonical_id.as_str() == source_id.as_str()
        }));
        assert!(!raw_rows.removed_ids.contains(source_id));
        let detail_ref = event.metadata["chat_detail_ref"]
            .as_str()
            .expect("each restored native row keeps its own source detail reference");
        let expanded = read_complete_detail_for_event(
            &context,
            &snapshot,
            &raw_rows.revision,
            detail_ref,
            source_id,
        );
        assert_eq!(expanded.as_bytes(), text.as_bytes());
    }

    let archived = archive.chat_events_for_agent("agent-1").unwrap();
    let archived_pair: Vec<_> = archived
        .iter()
        .filter(|event| event.id == live_id || event.id == completed_id)
        .collect();
    assert_eq!(archived_pair.len(), 2);
    assert_ne!(archived_pair[0].id, archived_pair[1].id);
}

fn trace_revalidation_actual_page(
    label: &str,
    requested_ids: Option<&[String]>,
    page: &AgentChatPage,
) {
    eprintln!(
        "CHAT_REVALIDATION_ACTUAL_PAGE label={label} requested_ids={} dto={}",
        serde_json::to_string(&requested_ids).unwrap(),
        serde_json::to_string(page).unwrap()
    );
}

fn loaded_source_ids_from_pages(
    agent_id: &str,
    inspected_id: &str,
    pages: &[&AgentChatPage],
) -> Vec<String> {
    let source_prefix = format!("source:{agent_id}:");
    // Select source observations only; provider and native turn IDs are not selectors.
    // Older pages are passed first, matching applyChatPage's chronological candidate window.
    let loaded_events: Vec<_> = pages.iter().flat_map(|page| page.events.iter()).collect();
    assert!(
        loaded_events.iter().any(|event| {
            loaded_source_ids_from_header(event, &source_prefix)
                .iter()
                .any(|source_id| source_id == inspected_id)
        }),
        "the inspected source observation is present in the loaded window"
    );

    let mut source_ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let oldest_loaded_row = loaded_events
        .iter()
        .find(|event| {
            event.metadata["optimistic"] != true
                && !loaded_source_ids_from_header(event, &source_prefix).is_empty()
        })
        .expect("the loaded window has an oldest source-backed row");
    append_loaded_source_ids_from_header(
        oldest_loaded_row,
        &source_prefix,
        &mut source_ids,
        &mut seen,
    );
    for event in loaded_events {
        append_loaded_source_ids_from_header(event, &source_prefix, &mut source_ids, &mut seen);
    }
    assert!(source_ids.iter().any(|source_id| source_id == inspected_id));
    source_ids
}

fn assert_loaded_source_ids_start_with_oldest_row(
    agent_id: &str,
    pages: &[&AgentChatPage],
    source_ids: &[String],
) {
    let source_prefix = format!("source:{agent_id}:");
    let oldest_loaded_row = pages
        .iter()
        .flat_map(|page| page.events.iter())
        .find(|event| {
            event.metadata["optimistic"] != true
                && !loaded_source_ids_from_header(event, &source_prefix).is_empty()
        })
        .expect("the loaded window has an oldest source-backed row");
    let anchor_ids = loaded_source_ids_from_header(oldest_loaded_row, &source_prefix);
    assert_eq!(
        source_ids.get(..anchor_ids.len()),
        Some(anchor_ids.as_slice()),
        "the renewal plan starts with the oldest loaded row's source members"
    );
}

fn append_loaded_source_ids_from_header(
    event: &AgentChatEvent,
    source_prefix: &str,
    source_ids: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    if event.metadata["optimistic"] == true {
        return;
    }
    for source_id in loaded_source_ids_from_header(event, source_prefix) {
        if seen.insert(source_id.clone()) {
            source_ids.push(source_id);
        }
    }
}

fn loaded_source_ids_from_header(event: &AgentChatEvent, source_prefix: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(members) = event.metadata["chat_display_member_ids"].as_array() {
        candidates.extend(members.iter().filter_map(|member| member.as_str()));
    }
    if let Some(source_ref) = event.metadata["chat_source_ref"].as_str() {
        candidates.push(source_ref);
    }
    if let Some(physical_id) = event.metadata["chat_display_physical_id"].as_str() {
        candidates.push(physical_id);
    }
    candidates.push(event.id.as_str());

    let mut seen = std::collections::BTreeSet::new();
    candidates
        .into_iter()
        .filter(|source_id| source_id.starts_with(source_prefix) && seen.insert(*source_id))
        .map(str::to_owned)
        .collect()
}

fn source_id_batches(source_ids: &[String]) -> Vec<Vec<String>> {
    assert!(source_ids
        .iter()
        .all(|source_id| source_id.len() <= super::MAX_REVALIDATED_SOURCE_ID_BYTES));
    let batches: Vec<_> = source_ids
        .chunks(super::MAX_REVALIDATED_SOURCE_IDS)
        .map(|batch| batch.to_vec())
        .collect();
    assert!(!batches.is_empty());
    assert!(batches.iter().all(|batch| {
        !batch.is_empty()
            && batch.len() <= super::MAX_REVALIDATED_SOURCE_IDS
            && batch
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == batch.len()
    }));
    batches
}

fn trace_revalidation_batch_plan(label: &str, source_ids: &[String], batches: &[Vec<String>]) {
    eprintln!(
        "CHAT_REVALIDATION_BATCH_PLAN label={label} requested_ids={} batches={}",
        serde_json::to_string(source_ids).unwrap(),
        serde_json::to_string(batches).unwrap()
    );
}

fn assert_revalidated_page_covers_ids(
    page: &AgentChatPage,
    source_ids: &[String],
    generation: &str,
    epoch: &str,
    admission: &str,
) {
    assert_eq!(page.generation.as_deref(), Some(generation));
    assert_eq!(page.source_epoch.as_deref(), Some(epoch));
    for source_id in source_ids {
        let event = page
            .events
            .iter()
            .find(|event| {
                event.id.as_str() == source_id.as_str()
                    || event.metadata["chat_source_ref"].as_str() == Some(source_id.as_str())
                    || event.metadata["chat_display_member_ids"]
                        .as_array()
                        .is_some_and(|members| {
                            members
                                .iter()
                                .any(|member| member.as_str() == Some(source_id))
                        })
            })
            .unwrap_or_else(|| {
                panic!(
                    "the actual response DTO must represent requested source observation {source_id}"
                )
            });
        assert_eq!(event.metadata["chat_source_epoch"].as_str(), Some(epoch));
        assert_eq!(
            event.metadata["chat_source_admission"].as_str(),
            Some(admission)
        );
    }
}

#[test]
fn revalidation_request_limits_raw_ids_before_deduplication() {
    let source_id = format!("source:agent-1:{}:10:{}:0", "a".repeat(64), "b".repeat(64));
    assert!(super::validate_revalidation_request(
        "agent-1",
        None,
        Some("current-revision"),
        None,
        Some(Vec::new()),
    )
    .is_err());
    let accepted = super::validate_revalidation_request(
        "agent-1",
        None,
        Some("current-revision"),
        None,
        Some(vec![source_id.clone(); super::MAX_REVALIDATED_SOURCE_IDS]),
    )
    .unwrap();
    assert_eq!(accepted, Some(vec![source_id.clone()]));
    assert!(super::validate_revalidation_request(
        "agent-1",
        None,
        Some("current-revision"),
        None,
        Some(vec![
            source_id.clone();
            super::MAX_REVALIDATED_SOURCE_IDS + 1
        ]),
    )
    .is_err());

    let oversized = format!("{source_id}{}", "x".repeat(512));
    assert!(super::validate_revalidation_request(
        "agent-1",
        None,
        Some("current-revision"),
        None,
        Some(vec![oversized]),
    )
    .is_err());
    assert!(super::validate_revalidation_request(
        "agent-1",
        Some("source-cursor"),
        Some("current-revision"),
        None,
        Some(vec![source_id.clone()]),
    )
    .is_err());
    assert!(super::validate_revalidation_request(
        "agent-1",
        None,
        Some("current-revision"),
        Some("detail-ref"),
        Some(vec![source_id.clone()]),
    )
    .is_err());
    assert!(super::validate_revalidation_request(
        "agent-1",
        None,
        None,
        None,
        Some(vec![source_id.clone()]),
    )
    .is_err());
    let foreign_id = source_id.replacen("source:agent-1:", "source:other-agent:", 1);
    assert!(super::validate_revalidation_request(
        "agent-1",
        None,
        Some("current-revision"),
        None,
        Some(vec![foreign_id]),
    )
    .is_err());
}

#[test]
fn revalidation_waits_for_a_complete_current_source_head_without_partial_acknowledgment() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("source-revalidation-incomplete.jsonl");
    let contents = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        commentary_stream_line(),
        commentary_item_line("msg-commentary-native")
    );
    let (snapshot, mut policy) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    finish_source_index(&snapshot);
    let qualified = read(&context, &snapshot, None, None, None).unwrap();
    let indexed = source_index_page(&snapshot);
    let live_id = indexed
        .events
        .iter()
        .find(|event| event.metadata["chat_display_member_ids"].is_array())
        .unwrap()
        .id
        .clone();
    let indexed_pair = indexed
        .events
        .iter()
        .find(|event| {
            event.id == live_id
                && event.source.as_deref() == Some("response_item")
                && event.metadata["provider_phase"] == "commentary"
                && event.metadata["provider_turn_id"] == "commentary-provider-turn"
                && event.metadata["chat_display_member_ids"]
                    .as_array()
                    .is_some_and(|members| {
                        members
                            .iter()
                            .any(|member| member.as_str() == Some(live_id.as_str()))
                    })
                && event.metadata["chat_display_physical_id"]
                    .as_str()
                    .is_some_and(|physical_id| physical_id != live_id.as_str())
        })
        .expect("the current certified pair identifies its distinct physical B row");
    let completed_id = indexed_pair.metadata["chat_display_physical_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(live_id, completed_id);
    let pair_members: BTreeSet<_> = indexed_pair.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        pair_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    let ids = vec![live_id.clone(), completed_id.clone()];

    append_native_line(
        &path,
        &json!({"type":"event_msg","payload":{"type":"user_message","id":"msg-unindexed-control","client_id":"unindexed-control","message":"await the source index"}})
            .to_string(),
    );
    let current_revision =
        read(&context, &snapshot, None, Some(&qualified.revision), None).unwrap();
    let pending = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&current_revision.revision),
        None,
        Some(ids.as_slice()),
    )
    .unwrap();
    assert!(pending.revalidated_ids.is_none());
    assert!(serde_json::to_value(&pending).unwrap()["revalidated_ids"].is_null());
    assert!(pending.events.is_empty());
    assert_eq!(pending.progress, "indexing");

    finish_source_index(&snapshot);
    let indexed_revision = read(
        &context,
        &snapshot,
        None,
        Some(&current_revision.revision),
        None,
    )
    .unwrap();
    let complete = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&indexed_revision.revision),
        None,
        Some(ids.as_slice()),
    )
    .unwrap();
    assert_eq!(complete.revalidated_ids, Some(ids.clone()));
    assert_eq!(complete.events.len(), 1);

    let duplicate_ids = vec![live_id.clone(), live_id.clone()];
    let deduplicated = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&complete.revision),
        None,
        Some(duplicate_ids.as_slice()),
    )
    .unwrap();
    assert_eq!(deduplicated.revalidated_ids, Some(vec![live_id.clone()]));

    policy.generation += 1;
    wardian_core::conversations::write_json_atomic(
        &super::policy_path(&context.agent_id).unwrap(),
        &Some(&policy),
    )
    .unwrap();
    assert!(!super::revalidation_page_is_current(&snapshot, &complete));
    let invalidated = super::read_with_revalidation(
        &context,
        &snapshot,
        None,
        Some(&indexed_revision.revision),
        None,
        Some(ids.as_slice()),
    )
    .unwrap();
    assert_eq!(invalidated.progress, "source_changed");
    assert!(invalidated.revalidated_ids.is_none());
    assert!(invalidated.events.is_empty());
}

fn trace_event_header(event: &AgentChatEvent) -> serde_json::Value {
    const METADATA_KEYS: &[&str] = &[
        "raw_type",
        "provider_log",
        "generated",
        "log_path",
        "source_path",
        "provider_session_id",
        "provider_turn_id",
        "provider_phase",
        "conversation_archive_id",
        "chat_provisional",
        "chat_source_ref",
        "chat_source_epoch",
        "chat_source_admission",
        "chat_source_start",
        "chat_source_end",
        "chat_display_physical_id",
        "chat_display_member_ids",
        "chat_display_removed_ids",
        "chat_page_key",
    ];
    let metadata = METADATA_KEYS
        .iter()
        .filter_map(|key| {
            event
                .metadata
                .get(*key)
                .cloned()
                .map(|value| ((*key).to_owned(), value))
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    let mut metadata_keys = event
        .metadata
        .as_object()
        .map(|metadata| metadata.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    metadata_keys.sort();
    json!({
        "id": event.id,
        "sequence": event.sequence,
        "source": event.source,
        "role": event.role.as_ref().map(|role| format!("{role:?}")),
        "turn_id": event.turn_id,
        "provider": event.provider,
        "metadata_keys": metadata_keys,
        "metadata": metadata,
    })
}

fn trace_stale_head_cursor_page(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    label: &str,
    page: &AgentChatPage,
    live_id: &str,
    completed_id: &str,
) {
    let returned_headers: Vec<_> = page.events.iter().map(trace_event_header).collect();
    let actual_page = serde_json::to_value(page).unwrap();
    let returned_header_bytes = serde_json::to_vec(&page.events).unwrap().len();
    let compact_returned_header_bytes = serde_json::to_vec(&returned_headers).unwrap().len();
    let current_source_scope = crate::commands::chat_recent_seed::source_scope(snapshot)
        .ok()
        .map(|(epoch, admission, extent, _)| {
            json!({"epoch": epoch, "admission": admission, "extent": extent})
        });
    let pair_events: Vec<_> = page
        .events
        .iter()
        .filter(|event| {
            event.id == live_id
                || event.id == completed_id
                || event.metadata["chat_display_physical_id"].as_str() == Some(completed_id)
        })
        .map(|event| {
            json!({
                "id": event.id,
                "source": event.source,
                "role": event.role,
                "turn_id": event.turn_id,
                "provider_phase": event.metadata["provider_phase"].clone(),
                "provider_turn_id": event.metadata["provider_turn_id"].clone(),
                "chat_source_ref": event.metadata["chat_source_ref"].clone(),
                "chat_source_epoch": event.metadata["chat_source_epoch"].clone(),
                "chat_source_admission": event.metadata["chat_source_admission"].clone(),
                "chat_source_start": event.metadata["chat_source_start"].clone(),
                "chat_source_end": event.metadata["chat_source_end"].clone(),
                "chat_display_physical_id": event.metadata["chat_display_physical_id"].clone(),
                "chat_display_member_ids": event.metadata["chat_display_member_ids"].clone(),
                "chat_display_removed_ids": event.metadata["chat_display_removed_ids"].clone(),
            })
        })
        .collect();
    let incoming_seed = match crate::commands::chat_recent_seed::read(snapshot, None) {
        Ok(seed) => {
            let headers: Vec<_> = seed.events.iter().map(trace_event_header).collect();
            let header_bytes = serde_json::to_vec(&headers).unwrap().len();
            json!({
                "progress": seed.progress,
                "revision": seed.revision,
                "unchanged": seed.unchanged,
                "bytes_read": seed.bytes_read,
                "records_decoded": seed.records_decoded,
                "checkpoint": seed.checkpoint.as_ref().map(|checkpoint| json!({
                    "epoch": checkpoint.epoch,
                    "admission": checkpoint.admission,
                    "extent": checkpoint.extent,
                    "before": checkpoint.before,
                    "window_complete": checkpoint.window_complete,
                    "normalizer_complete": checkpoint.normalizer_complete,
                })),
                "header_count": headers.len(),
                "serialized_header_bytes": header_bytes,
                "headers": headers,
            })
        }
        Err(error) => json!({"read_error": error.to_string()}),
    };
    let aliases: Vec<_> = page
        .aliases
        .iter()
        .filter(|alias| alias.observation_id == live_id || alias.observation_id == completed_id)
        .map(|alias| {
            json!({
                "observation_id": alias.observation_id,
                "canonical_id": alias.canonical_id,
            })
        })
        .collect();
    let removed_ids: Vec<_> = page
        .removed_ids
        .iter()
        .filter(|id| id.as_str() == live_id || id.as_str() == completed_id)
        .cloned()
        .collect();
    eprintln!(
        "CHAT1693_OLDER_RETENTION_RUST_DTO {label} {}",
        json!({
            "session_id": page.session_id,
            "conversation_id": page.conversation_id,
            "generation": page.generation,
            "source_epoch": page.source_epoch,
            "revision": page.revision,
            "event_count": page.events.len(),
            "next_before": page.next_before,
            "reset": page.reset,
            "progress": page.progress,
            "unchanged": page.unchanged,
            "actual_page": actual_page,
            "returned_header_count": page.events.len(),
            "returned_serialized_header_bytes": returned_header_bytes,
            "compact_returned_header_bytes": compact_returned_header_bytes,
            "returned_headers": returned_headers,
            "current_source_scope": current_source_scope,
            "current_native_seed_snapshot": incoming_seed,
            "pair_events": pair_events,
            "aliases": aliases,
            "removed_ids": removed_ids,
            "bytes_read": page.bytes_read,
            "records_decoded": page.records_decoded,
        })
    );
}

fn assert_current_commentary_source_index_proof(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    live_id: &str,
    completed_id: &str,
    current_extent: u64,
) -> (String, String, (u64, u64), usize) {
    let (epoch, admission, extent, _) =
        crate::commands::chat_recent_seed::source_scope(snapshot).unwrap();
    assert_eq!(extent, current_extent);
    assert_eq!(
        fs::metadata(snapshot.log_path.as_ref().unwrap())
            .unwrap()
            .len(),
        current_extent
    );

    let expected_members = BTreeSet::from([live_id, completed_id]);
    let mut cursor = None;
    let mut generation = None;
    let mut pair_source_refs = BTreeSet::new();
    let mut pair_rows = 0;
    let mut native_tool_call_rows = 0;
    let mut completed_source_span = None;
    let mut reached_end = false;
    let mut pages_visited = 0usize;
    for page_number in 0..8 {
        let cursor_before = cursor.clone();
        let page = source_index_page_with_cursor(snapshot, cursor_before.as_deref());
        pages_visited += 1;
        let pair_rows_on_page: Vec<_> = page
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.metadata["chat_source_ref"].as_str(),
                    Some(source_ref) if source_ref == live_id || source_ref == completed_id
                )
            })
            .map(|event| {
                json!({
                    "id": event.id,
                    "source_ref": event.metadata["chat_source_ref"],
                    "start": event.metadata["chat_source_start"],
                    "end": event.metadata["chat_source_end"],
                    "physical_id": event.metadata["chat_display_physical_id"],
                    "member_ids": event.metadata["chat_display_member_ids"],
                })
            })
            .collect();
        eprintln!(
            "CHAT1693_SOURCE_PAGE_WALK {}",
            json!({
                "page_number": page_number,
                "cursor_before": cursor_before,
                "next_before": page.next_before,
                "event_count": page.events.len(),
                "counts_complete": page.counts_complete,
                "first_source_start": page
                    .events
                    .first()
                    .and_then(|event| event.metadata["chat_source_start"].as_u64()),
                "last_source_start": page
                    .events
                    .last()
                    .and_then(|event| event.metadata["chat_source_start"].as_u64()),
                "pair_rows": pair_rows_on_page,
            })
        );
        assert!(
            page.counts_complete,
            "the source index covers the full admitted span"
        );
        assert_eq!(page.epoch, epoch);
        assert_eq!(page.admission, admission);
        assert_eq!(page.target_extent, current_extent);
        if let Some(expected_generation) = generation.as_deref() {
            assert_eq!(page.generation, expected_generation);
        } else {
            generation = Some(page.generation.clone());
        }

        for event in &page.events {
            let source_ref = event.metadata["chat_source_ref"].as_str().unwrap();
            assert_eq!(event.metadata["chat_source_epoch"], json!(epoch));
            assert_eq!(event.metadata["chat_source_admission"], json!(admission));
            let start = event.metadata["chat_source_start"].as_u64().unwrap();
            let end = event.metadata["chat_source_end"].as_u64().unwrap();
            assert!(start < end && end <= current_extent);

            if event.kind == AgentChatEventKind::ToolCall {
                native_tool_call_rows += 1;
                assert!(event.metadata.get("chat_display_member_ids").is_none());
                assert!(event.metadata.get("chat_display_physical_id").is_none());
            }

            if source_ref == live_id || source_ref == completed_id {
                pair_rows += 1;
                pair_source_refs.insert(source_ref.to_owned());
                if source_ref == completed_id {
                    let previous_span = completed_source_span.replace((start, end));
                    assert!(
                        previous_span.is_none(),
                        "the completed source row appeared on multiple pages: page={page_number} cursor_before={cursor_before:?} next_before={:?} current=({start},{end}) previous={previous_span:?}",
                        page.next_before
                    );
                }
                assert_eq!(event.id, live_id);
                assert_eq!(
                    event.metadata["chat_display_physical_id"].as_str(),
                    Some(source_ref)
                );
                assert_eq!(
                    event.metadata["chat_display_member_ids"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|id| id.as_str())
                        .collect::<BTreeSet<_>>(),
                    expected_members
                );
                assert_eq!(event.metadata["provider_phase"], "commentary");
                assert_eq!(
                    event.metadata["provider_turn_id"],
                    "commentary-provider-turn"
                );
            }
        }

        cursor = page.next_before;
        if cursor.is_none() {
            reached_end = true;
            break;
        }
    }
    assert!(
        reached_end,
        "the bounded fixture fits within eight source-index pages; pages_visited={pages_visited} final_before={cursor:?}"
    );
    assert_eq!(pair_rows, 2);
    assert_eq!(
        pair_source_refs,
        BTreeSet::from([live_id.to_owned(), completed_id.to_owned()])
    );
    (
        epoch,
        admission,
        completed_source_span.expect("the current complete source index contains native B"),
        native_tool_call_rows,
    )
}

fn assert_incomplete_source_index_withholds_commentary_pair(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    live_id: &str,
    completed_id: &str,
) {
    let (epoch, admission, extent, _) =
        crate::commands::chat_recent_seed::source_scope(snapshot).unwrap();
    let page = source_index_page(snapshot);
    assert!(!page.counts_complete);
    assert_eq!(page.epoch, epoch);
    let pair_rows: Vec<_> = page
        .events
        .iter()
        .filter(|event| {
            matches!(
                event.metadata["chat_source_ref"].as_str(),
                Some(source_ref) if source_ref == live_id || source_ref == completed_id
            )
        })
        .collect();
    assert_eq!(pair_rows.len(), 2);
    for event in pair_rows {
        let source_ref = event.metadata["chat_source_ref"].as_str().unwrap();
        assert_eq!(event.metadata["chat_source_admission"], json!(admission));
        assert!(event.metadata["chat_source_end"].as_u64().unwrap() <= extent);
        assert_eq!(event.id, source_ref);
        assert!(event.metadata.get("chat_display_member_ids").is_none());
        assert!(event.metadata.get("chat_display_physical_id").is_none());
    }
}

#[test]
fn stale_head_older_cursor_reconciles_qualified_commentary_pair_inside_page() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("stale-head-pair-in-older-page.jsonl");
    let contents = format!(
        "{}\n{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        commentary_context_line(),
        commentary_stream_line()
    );
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    let live_id = first
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("event_msg")
                && event.text.as_deref() == Some("A source-bound commentary observation.")
        })
        .unwrap()
        .id
        .clone();
    archive
        .append_provider_log_batch_with_context(context.clone(), &first.events, None, &first.next)
        .unwrap();
    finish_projection(&archive);

    let first_publication = read(&context, &snapshot, None, None, None).unwrap();
    let first_pair: Vec<_> = first_publication
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("A source-bound commentary observation."))
        .collect();
    assert_eq!(first_pair.len(), 1);
    assert_eq!(first_pair[0].id, live_id);

    let completion_line = commentary_item_line("msg-commentary-native");
    append_native_line(&path, &completion_line);
    let mut completion = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(first.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut completion.events,
        "codex",
        &path,
    );
    let completed_id = completion
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("response_item")
                && event.text.as_deref() == Some("A source-bound commentary observation.")
        })
        .unwrap()
        .id
        .clone();
    assert_eq!(completion.events.len(), 1);
    archive
        .append_provider_log_batch_with_context(
            context.clone(),
            &completion.events,
            Some(&first.next),
            &completion.next,
        )
        .unwrap();
    finish_projection(&archive);
    finish_source_index(&snapshot);

    let qualified = read(
        &context,
        &snapshot,
        None,
        Some(&first_publication.revision),
        None,
    )
    .unwrap();
    let qualified_pair: Vec<_> = qualified
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("A source-bound commentary observation."))
        .collect();
    assert_eq!(qualified_pair.len(), 1);
    assert_eq!(qualified_pair[0].id, live_id);
    let qualified_members: BTreeSet<_> = qualified_pair[0].metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        qualified_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    let old_head_ref = super::head("agent-1").unwrap().unwrap();
    let old_head: super::Head = super::store("agent-1")
        .unwrap()
        .read(&old_head_ref)
        .unwrap();
    let old_extent = fs::metadata(&path).unwrap().len();
    assert!(old_head.logical.has_evidence());
    assert_eq!(old_head.logical.completed_watermark(), Some(old_extent));

    // Exactly one full recent page follows the pair, putting both native
    // observations in the next page returned by the real source cursor.
    for index in 0..80 {
        append_native_line(
            &path,
            &json!({
                "type":"event_msg",
                "payload":{"type":"user_message","id":format!("older-fill-{index}"),"message":format!("older filler {index}")}
            })
            .to_string(),
        );
    }
    let current_extent = fs::metadata(&path).unwrap().len();
    assert!(current_extent > old_extent);
    finish_source_index(&snapshot);
    assert_eq!(
        super::head("agent-1").unwrap().as_deref(),
        Some(old_head_ref.as_str())
    );
    let stale_head: super::Head = super::store("agent-1")
        .unwrap()
        .read(&old_head_ref)
        .unwrap();
    assert_eq!(stale_head.logical.completed_watermark(), Some(old_extent));
    let (epoch, admission, source_index_completed_span, _) =
        assert_current_commentary_source_index_proof(
            &snapshot,
            &live_id,
            &completed_id,
            current_extent,
        );
    assert_eq!(stale_head.source_epoch.as_deref(), Some(epoch.as_str()));
    assert!(stale_head.logical.admits(&admission));
    let current_source_page = source_index_page(&snapshot);
    assert!(current_source_page.counts_complete);
    assert_eq!(current_source_page.epoch, epoch);
    assert!(current_source_page.next_before.is_some());
    assert!(!current_source_page.events.iter().any(|event| {
        matches!(
            event.metadata["chat_source_ref"].as_str(),
            Some(source_ref) if source_ref == live_id || source_ref == completed_id
        )
    }));

    let recent = read(&context, &snapshot, None, Some(&qualified.revision), None).unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "pair-outside-first-page",
        &recent,
        &live_id,
        &completed_id,
    );
    let cursor = recent
        .next_before
        .as_deref()
        .expect("the real bounded recent page returns an older source cursor");
    assert!(cursor.starts_with("source:"));
    let older = read(
        &context,
        &snapshot,
        Some(cursor),
        Some(&recent.revision),
        None,
    )
    .unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "pair-inside-older-page",
        &older,
        &live_id,
        &completed_id,
    );

    assert_eq!(older.session_id, context.agent_id);
    assert_eq!(older.conversation_id, qualified.conversation_id);
    assert_eq!(older.generation, qualified.generation);
    assert_eq!(older.source_epoch, qualified.source_epoch);
    assert!(older.events.len() <= 80);
    assert!(older.bytes_read < 2 * 1024 * 1024);
    assert!(older.records_decoded < 640);
    let pair: Vec<_> = older
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("A source-bound commentary observation."))
        .collect();
    assert_eq!(pair.len(), 1);
    let displayed = pair[0];
    assert_eq!(displayed.id, live_id);
    assert_eq!(displayed.source.as_deref(), Some("response_item"));
    assert_eq!(displayed.metadata["provider_phase"], "commentary");
    assert_eq!(
        displayed.metadata["provider_turn_id"],
        "commentary-provider-turn"
    );
    assert_eq!(
        displayed.metadata["chat_display_physical_id"],
        json!(completed_id)
    );
    assert_eq!(
        displayed.metadata["chat_source_ref"].as_str(),
        Some(completed_id.as_str())
    );
    assert_eq!(displayed.metadata["chat_source_epoch"], json!(epoch));
    let completed_event = completion
        .events
        .iter()
        .find(|event| event.id == completed_id)
        .unwrap();
    let acquisition_start = completed_event.metadata["chat_source_start"]
        .as_u64()
        .unwrap();
    let acquisition_end = completed_event.metadata["chat_source_end"]
        .as_u64()
        .unwrap();
    let (source_index_start, source_index_end) = source_index_completed_span;
    let raw_log = fs::read(&path).unwrap();
    let acquired_frame = &raw_log[acquisition_start as usize..acquisition_end as usize];
    assert_eq!(acquired_frame.last(), Some(&b'\n'));
    let acquired_json = acquired_frame.strip_suffix(b"\n").unwrap();
    let acquired_json = acquired_json.strip_suffix(b"\r").unwrap_or(acquired_json);
    assert_eq!(acquired_json, completion_line.as_bytes());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(acquired_json).unwrap(),
        serde_json::from_str::<serde_json::Value>(&completion_line).unwrap()
    );
    let indexed_frame = &raw_log[source_index_start as usize..source_index_end as usize];
    assert_eq!(indexed_frame, acquired_frame);
    let indexed_json = indexed_frame.strip_suffix(b"\n").unwrap();
    let indexed_json = indexed_json.strip_suffix(b"\r").unwrap_or(indexed_json);
    assert_eq!(indexed_json, completion_line.as_bytes());
    assert_eq!(source_index_start, acquisition_start);
    assert_eq!(source_index_end, acquisition_end);
    let framing_bytes = acquired_frame.len() - completion_line.len();
    assert!(matches!(framing_bytes, 1 | 2));
    assert_eq!(
        source_index_end,
        source_index_start + completion_line.len() as u64 + framing_bytes as u64
    );
    assert_eq!(
        displayed.metadata["chat_source_start"].as_u64(),
        Some(source_index_start)
    );
    assert_eq!(
        displayed.metadata["chat_source_end"].as_u64(),
        Some(source_index_end)
    );
    let members: BTreeSet<_> = displayed.metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    assert!(older
        .aliases
        .iter()
        .any(|alias| { alias.observation_id == completed_id && alias.canonical_id == live_id }));
    assert!(!older.removed_ids.contains(&live_id));

    let archived = archive.chat_events_for_agent("agent-1").unwrap();
    for (observed_id, acquired_event) in [
        (
            live_id.as_str(),
            first
                .events
                .iter()
                .find(|event| event.id == live_id)
                .unwrap(),
        ),
        (completed_id.as_str(), completed_event),
    ] {
        let archived_event = archived
            .iter()
            .find(|event| event.id == observed_id)
            .unwrap();
        assert_eq!(
            archived_event.metadata["chat_source_start"],
            acquired_event.metadata["chat_source_start"]
        );
        assert_eq!(
            archived_event.metadata["chat_source_end"],
            acquired_event.metadata["chat_source_end"]
        );
    }
    let raw_ids: BTreeSet<_> = archived
        .iter()
        .filter(|event| event.kind == AgentChatEventKind::Message)
        .map(|event| event.id.as_str())
        .collect();
    assert_eq!(
        raw_ids,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
}

#[test]
fn stale_head_older_cursor_records_loaded_pair_outside_page_dto() {
    let (_guard, temp) = isolated_home();
    let path = temp.path().join("stale-head-pair-outside-older-page.jsonl");
    let mut contents = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}})
    );
    for index in 0..76 {
        contents.push_str(&native_message(&format!("older-history-{index}")));
    }
    contents.push_str(&format!(
        "{}\n{}\n",
        commentary_context_line(),
        commentary_stream_line()
    ));
    let (snapshot, _) = owned_native_source(&path, &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    let archive = ConversationArchiveState::default();
    let mut first = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        None,
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(&mut first.events, "codex", &path);
    let live_id = first
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("event_msg")
                && event.text.as_deref() == Some("A source-bound commentary observation.")
        })
        .unwrap()
        .id
        .clone();
    archive
        .append_provider_log_batch_with_context(context.clone(), &first.events, None, &first.next)
        .unwrap();
    finish_projection(&archive);

    let first_publication = read(&context, &snapshot, None, None, None).unwrap();
    let first_pair: Vec<_> = first_publication
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("A source-bound commentary observation."))
        .collect();
    assert_eq!(first_pair.len(), 1);
    assert_eq!(first_pair[0].id, live_id);

    append_native_line(&path, &commentary_item_line("msg-commentary-native"));
    let mut completion = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(first.next.clone()),
        true,
    )
    .unwrap();
    crate::commands::chat::decorate_forward_provider_log_events(
        &mut completion.events,
        "codex",
        &path,
    );
    let completed_id = completion
        .events
        .iter()
        .find(|event| {
            event.source.as_deref() == Some("response_item")
                && event.text.as_deref() == Some("A source-bound commentary observation.")
        })
        .unwrap()
        .id
        .clone();
    assert_eq!(completion.events.len(), 1);
    archive
        .append_provider_log_batch_with_context(
            context.clone(),
            &completion.events,
            Some(&first.next),
            &completion.next,
        )
        .unwrap();
    let tail_start = completion.next.clone();
    for index in 0..3 {
        append_native_line(
            &path,
            &json!({
                "type":"event_msg",
                "payload":{"type":"user_message","id":format!("index-tail-{index}"),"message":format!("Index tail {index}")}
            })
            .to_string(),
        );
    }
    let mut tail = crate::commands::provider_log_acquisition::acquire_provider_log_batch(
        "agent-1",
        "codex",
        &path,
        context.provider_source_key.as_deref().unwrap(),
        Some(tail_start.clone()),
        true,
    )
    .unwrap();
    assert_eq!(tail.events.len(), 3);
    crate::commands::chat::decorate_forward_provider_log_events(&mut tail.events, "codex", &path);
    archive
        .append_provider_log_batch_with_context(
            context.clone(),
            &tail.events,
            Some(&tail_start),
            &tail.next,
        )
        .unwrap();
    finish_projection(&archive);

    // One bounded checkpoint reaches the pair and stops before the third tail
    // row, so the independently scanned prefix is useful but incomplete.
    assert!(super::advance_source_index(&snapshot).unwrap());
    assert_incomplete_source_index_withholds_commentary_pair(&snapshot, &live_id, &completed_id);
    finish_source_index(&snapshot);

    let qualified = read(
        &context,
        &snapshot,
        None,
        Some(&first_publication.revision),
        None,
    )
    .unwrap();
    let loaded_pair: Vec<_> = qualified
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("A source-bound commentary observation."))
        .collect();
    assert_eq!(loaded_pair.len(), 1);
    assert_eq!(loaded_pair[0].id, live_id);
    let loaded_members: BTreeSet<_> = loaded_pair[0].metadata["chat_display_member_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert_eq!(
        loaded_members,
        BTreeSet::from([live_id.as_str(), completed_id.as_str()])
    );
    let old_head_ref = super::head("agent-1").unwrap().unwrap();
    let old_head: super::Head = super::store("agent-1")
        .unwrap()
        .read(&old_head_ref)
        .unwrap();
    let old_extent = fs::metadata(&path).unwrap().len();
    assert!(old_head.logical.has_evidence());
    assert_eq!(old_head.logical.completed_watermark(), Some(old_extent));

    append_native_line(
        &path,
        &json!({
            "type":"event_msg",
            "payload":{"type":"user_message","id":"stale-head-tail-control","message":"A separate native tail control."}
        })
        .to_string(),
    );
    let current_extent = fs::metadata(&path).unwrap().len();
    assert!(current_extent > old_extent);
    finish_source_index(&snapshot);
    assert_eq!(
        super::head("agent-1").unwrap().as_deref(),
        Some(old_head_ref.as_str())
    );
    let (epoch, admission, _source_index_completed_span, _) =
        assert_current_commentary_source_index_proof(
            &snapshot,
            &live_id,
            &completed_id,
            current_extent,
        );
    assert_eq!(old_head.source_epoch.as_deref(), Some(epoch.as_str()));
    assert!(old_head.logical.admits(&admission));

    let recent = read(&context, &snapshot, None, Some(&qualified.revision), None).unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "loaded-pair-recent-page",
        &recent,
        &live_id,
        &completed_id,
    );
    let recent_pair: Vec<_> = recent
        .events
        .iter()
        .filter(|event| event.text.as_deref() == Some("A source-bound commentary observation."))
        .collect();
    assert_eq!(recent_pair.len(), 1);
    assert_eq!(recent_pair[0].id, live_id);
    assert_eq!(
        recent_pair[0].metadata["chat_display_member_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let cursor = recent
        .next_before
        .as_deref()
        .expect("the ordinary returned page has older history");
    let older = read(
        &context,
        &snapshot,
        Some(cursor),
        Some(&recent.revision),
        None,
    )
    .unwrap();
    trace_stale_head_cursor_page(
        &snapshot,
        "loaded-pair-outside-older-page",
        &older,
        &live_id,
        &completed_id,
    );

    // An older-page DTO need not repeat the already loaded pair. The marker
    // above records the real reset/alias/removal envelope used by the client.
    assert!(older.events.len() <= 80);
    assert!(older.bytes_read < 2 * 1024 * 1024);
    assert!(older.records_decoded < 640);
    assert!(!older.events.iter().any(|event| {
        event.id == live_id
            || event.id == completed_id
            || event.metadata["chat_display_physical_id"].as_str() == Some(completed_id.as_str())
    }));
    let archived = archive.chat_events_for_agent("agent-1").unwrap();
    assert!(archived.iter().any(|event| event.id == live_id));
    assert!(archived.iter().any(|event| event.id == completed_id));
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
    legacy
        .metadata
        .as_object_mut()
        .unwrap()
        .remove(crate::providers::chat_transcript::PROVIDER_EVENT_ID_METADATA_KEY);
    legacy.id =
        crate::commands::chat::archive_identity::legacy_provider_log_event_id(&legacy, path);
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
fn source_older_falls_back_from_sparse_prefix_at_recent_physical_frontier() {
    let (_guard, temp) = isolated_home();
    let mut contents = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}})
    );
    for index in 0..300 {
        contents.push_str(&native_message(&format!("sparse-message-{index}")));
    }
    let (snapshot, _) = owned_native_source(&temp.path().join("sparse-source.jsonl"), &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    assert!(advance_source_index(&snapshot).unwrap());

    let recent = read(&context, &snapshot, None, None, None).unwrap();
    assert_eq!(recent.events.len(), 80);
    assert_eq!(
        recent.events.first().unwrap().text.as_deref(),
        Some("sparse-message-220")
    );
    assert_eq!(
        recent.events.last().unwrap().text.as_deref(),
        Some("sparse-message-299")
    );
    let frontier = recent
        .events
        .iter()
        .min_by_key(|event| event.metadata["chat_source_start"].as_u64())
        .unwrap();
    let frontier_offset = frontier.metadata["chat_source_start"].as_u64().unwrap();
    let frontier_ordinal: u64 = frontier.id.rsplit(':').next().unwrap().parse().unwrap();
    let cursor = recent.next_before.as_deref().unwrap();
    assert!(cursor.ends_with(&format!("{frontier_offset:020}:{frontier_ordinal:020}")));
    let source_head_ref = super::super::chat_source_index::pointer("agent-1")
        .unwrap()
        .unwrap();
    let seed_checkpoint = crate::commands::chat_recent_seed::read(&snapshot, None)
        .unwrap()
        .checkpoint
        .unwrap();
    let mut source_store = super::store("agent-1").unwrap();
    assert!(
        super::super::chat_source_index::continuation(
            &snapshot,
            &mut source_store,
            &source_head_ref,
            &seed_checkpoint,
            None,
        )
        .unwrap()
        .is_none(),
        "a page without a selected physical frontier has no fabricated cursor"
    );
    let source_head: serde_json::Value = source_store.read(&source_head_ref).unwrap();
    assert!(
        source_head["admitted_through"]
            .as_u64()
            .is_none_or(|admitted_through| admitted_through < frontier_offset),
        "the forward prefix does not cover the selected recent frontier"
    );

    let older = read(
        &context,
        &snapshot,
        Some(cursor),
        Some(&recent.revision),
        None,
    )
    .unwrap();
    let expected = (140..220)
        .map(|index| format!("sparse-message-{index}"))
        .collect::<Vec<_>>();
    let actual = older
        .events
        .iter()
        .map(|event| event.text.clone().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert!(older.next_before.is_some());
    assert_eq!(older.progress, "indexing");
    assert!(older.bytes_read < 2 * 1024 * 1024);
    assert!(older.records_decoded < 512);
}

#[test]
fn older_seed_fallback_holds_its_cursor_at_an_unframed_gap() {
    let (_guard, temp) = isolated_home();
    let mut contents = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}})
    );
    for index in 0..300 {
        if index == 150 {
            contents.push_str("{malformed source record}\n");
        }
        contents.push_str(&native_message(&format!("gap-message-{index}")));
    }
    let (snapshot, _) = owned_native_source(&temp.path().join("gap-source.jsonl"), &contents);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    assert!(advance_source_index(&snapshot).unwrap());
    finish_source_index(&snapshot);
    assert!(!source_index_page(&snapshot).counts_complete);

    let recent = read(&context, &snapshot, None, None, None).unwrap();
    assert_eq!(recent.events.len(), 80);
    assert_eq!(
        recent.events.first().unwrap().text.as_deref(),
        Some("gap-message-220")
    );
    let cursor = recent.next_before.clone().unwrap();
    assert!(cursor.starts_with("source:"));

    let older = read(
        &context,
        &snapshot,
        Some(&cursor),
        Some(&recent.revision),
        None,
    )
    .unwrap();
    let actual = older
        .events
        .iter()
        .map(|event| event.text.as_deref().unwrap().to_owned())
        .collect::<Vec<_>>();
    let expected = (150..220)
        .map(|index| format!("gap-message-{index}"))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(older.next_before.as_deref(), Some(cursor.as_str()));
    assert!(!older.reset);
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
