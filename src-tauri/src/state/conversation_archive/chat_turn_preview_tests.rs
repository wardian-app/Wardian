use super::*;
use serde_json::json;

fn scope() -> Scope<'static> {
    Scope {
        agent_id: "synthetic-agent",
        conversation_id: "synthetic-conversation",
        source_key: Some("synthetic-source"),
        source_epoch: Some("synthetic-epoch"),
        can_activate: true,
    }
}

fn event(ordinal: u64, kind: AgentChatEventKind) -> AgentChatEvent {
    serde_json::from_value(json!({
        "id": format!("projected-{ordinal}"),
        "session_id": scope().agent_id,
        "provider": "codex",
        "kind": kind,
        "role": "assistant",
        "text": format!("Synthetic content {ordinal}"),
        "sequence": ordinal,
        "metadata": {
            "request_root_id": "request-root",
            "provider_turn_id": "provider-turn",
            "chat_turn_adapter": "codex_bracket_v1",
            "chat_turn_membership_complete": false,
            "chat_turn_membership_partial": false,
            "chat_source_ref": format!("native-{ordinal}"),
            "chat_source_start": ordinal * 100,
            "chat_source_end": ordinal * 100 + 50,
            "chat_source_epoch": scope().source_epoch,
        },
    }))
    .expect("synthetic normalized event")
}

fn prompt() -> AgentChatEvent {
    let mut event = event(1, AgentChatEventKind::Message);
    event.role = Some(AgentChatRole::User);
    event.metadata["input_origin"] = json!("human_input");
    event.metadata["input_purpose"] = json!("request");
    event
}

fn tool(ordinal: u64, result: bool) -> AgentChatEvent {
    let mut event = event(
        ordinal,
        if result {
            AgentChatEventKind::ToolResult
        } else {
            AgentChatEventKind::ToolCall
        },
    );
    event.metadata["call_id"] = json!("source-call");
    event.metadata["tool_name"] = json!("exec_command");
    event.metadata["tool_input"] = json!({"description": "Run synthetic tests"});
    event.status = Some(if result {
        AgentChatStatus::Succeeded
    } else {
        AgentChatStatus::Running
    });
    event
}

fn answer() -> AgentChatEvent {
    let mut event = event(4, AgentChatEventKind::Message);
    event.metadata["provider_phase"] = json!("final_answer");
    event.status = Some(AgentChatStatus::Succeeded);
    event
}

fn closure() -> AgentChatEvent {
    let mut event = event(5, AgentChatEventKind::Status);
    event.metadata["raw_type"] = json!("task_complete");
    event.metadata["chat_turn_membership_complete"] = json!(true);
    event.status = Some(AgentChatStatus::Succeeded);
    event
}

fn present(
    events: &[AgentChatEvent],
    known: &BTreeSet<String>,
    running: bool,
) -> Option<ChatPresentationV1> {
    preview(
        &scope(),
        events,
        "synthetic-admission",
        known,
        running,
        &PreviewOwners::default(),
    )
    .unwrap()
}

#[test]
fn preview_keys_equal_the_shared_canonical_derivation_and_original_hash_recipe() {
    let events = [prompt(), tool(2, false), tool(3, true), answer()];
    let page = present(&events, &BTreeSet::new(), true).unwrap();
    let owner = key(&("turn-owner-v1", scope().agent_id, scope().conversation_id)).unwrap();
    let source = key(&(
        "turn-source-v1",
        &owner,
        scope().source_key,
        scope().source_epoch,
    ))
    .unwrap();
    let turn_key = key(&("turn-v1", &source, "request-root")).unwrap();
    assert_eq!(page.turns[0].turn_key, turn_key);
    let expected = [
        key(&("prompt-v1", &turn_key)).unwrap(),
        key(&("step-v1", &turn_key, "source-call")).unwrap(),
        key(&("step-v1", &turn_key, "source-call")).unwrap(),
        key(&("item-v1", &turn_key, "native-4")).unwrap(),
    ];
    for (event, expected) in events.iter().zip(expected) {
        let kind = item_kind(event).unwrap();
        let identity = identity(&scope(), event, "request-root", &kind, Some(&owner))
            .unwrap()
            .unwrap();
        assert_eq!(identity.item_key, expected);
        assert_eq!(identity.source, source);
        assert_eq!(identity.turn_key, turn_key);
        assert_eq!(
            page.items
                .iter()
                .find(|item| item.event_refs.contains(&event.id))
                .unwrap()
                .item_key,
            identity.item_key
        );
    }
}

#[test]
fn canonical_identity_retains_event_fallback_and_foreign_scope_error_before_physical_validation() {
    let mut event = answer();
    event
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("chat_source_ref");
    let identity = identity(
        &scope(),
        &event,
        "request-root",
        &ChatPresentedItemKind::Answer,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(identity.physical, event.id);
    assert_eq!(
        identity.item_key,
        key(&("item-v1", &identity.turn_key, &event.id)).unwrap()
    );
    event.metadata["chat_source_ref"] = json!("x".repeat(ID_BYTES + 1));
    assert!(super::identity(
        &scope(),
        &event,
        "request-root",
        &ChatPresentedItemKind::Answer,
        None
    )
    .unwrap()
    .is_none());
    assert!(super::identity(
        &scope(),
        &event,
        "request-root",
        &ChatPresentedItemKind::Answer,
        Some("foreign-owner")
    )
    .is_err());
}

#[test]
fn running_then_closed_reuses_keys_order_and_paired_step_with_no_early_answer() {
    let mut events = vec![prompt(), tool(2, false), tool(3, true), answer()];
    let running = present(&events, &BTreeSet::new(), true).unwrap();
    let turn = &running.turns[0];
    assert!(turn.prefix_attributed);
    assert!(!turn.membership_complete && !turn.counts_complete && !turn.references_complete);
    assert_eq!(turn.status, ChatTurnStatus::Running);
    assert!(!turn.answer_recorded && turn.answer_item_keys.is_empty());
    assert_eq!(running.items[2].kind, ChatPresentedItemKind::Narration);
    assert_eq!(running.items[1].event_refs, ["projected-2", "projected-3"]);
    assert_eq!(
        running.items[1].step.as_ref().unwrap().description,
        "Run synthetic tests"
    );
    assert_eq!(
        running.items[1].step.as_ref().unwrap().status,
        ChatStepStatus::Completed
    );
    assert_eq!(
        running.items[1].preview.as_ref().unwrap().text,
        "Synthetic content 3"
    );
    events.push(closure());
    let closed = present(&events, &BTreeSet::new(), false).unwrap();
    assert_eq!(closed.turns[0].turn_key, turn.turn_key);
    assert_eq!(closed.turns[0].order_key, turn.order_key);
    assert!(closed.turns[0].membership_complete && closed.turns[0].answer_recorded);
    assert_eq!(closed.turns[0].status, ChatTurnStatus::Completed);
    assert_eq!(closed.items[2].kind, ChatPresentedItemKind::Answer);
    for (before, after) in running.items.iter().zip(&closed.items) {
        assert_eq!(
            (&before.item_key, &before.order_key),
            (&after.item_key, &after.order_key)
        );
    }
    assert_eq!(closed.state_event_refs.len(), 1);
    assert_eq!(closed.state_event_refs[0].event_id, "projected-5");
}

#[test]
fn missing_or_unsupported_middle_item_rejects_whole_prefix() {
    let mut middle = tool(2, false);
    middle
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("request_root_id");
    assert!(present(&[prompt(), middle, answer()], &BTreeSet::new(), true).is_none());
    let mut middle = event(2, AgentChatEventKind::Message);
    middle.role = Some(AgentChatRole::System);
    assert!(present(&[prompt(), middle, answer()], &BTreeSet::new(), true).is_none());
    let mut middle = tool(2, false);
    middle.metadata["chat_turn_membership_partial"] = json!(true);
    assert!(present(&[prompt(), middle, closure()], &BTreeSet::new(), false).is_none());
    let mut contradiction = tool(2, false);
    contradiction.metadata["chat_turn_membership_contradiction"] = json!(true);
    assert!(present(&[prompt(), contradiction, answer()], &BTreeSet::new(), true).is_none());
}

#[test]
fn exact_opening_or_same_admission_known_root_is_required_even_after_closure() {
    let events = [tool(2, false), tool(3, true), answer(), closure()];
    assert!(present(&events, &BTreeSet::new(), false).is_none());
    assert!(present(&events, &BTreeSet::from(["another-root".into()]), false).is_none());
    let known = BTreeSet::from(["request-root".into()]);
    assert!(
        present(&events, &known, false).is_none(),
        "an opening root alone cannot invent global order"
    );
    let opening = present(&[prompt()], &BTreeSet::new(), true).unwrap();
    let owners = PreviewOwners::from_presentation(&scope(), "synthetic-admission", &opening)
        .unwrap()
        .unwrap();
    let value = preview(
        &scope(),
        &events,
        "synthetic-admission",
        &known,
        false,
        &owners,
    )
    .unwrap()
    .unwrap();
    assert!(value.turns[0].membership_complete);
    assert!(value.turns[0].prompt_item_keys.is_empty());
    assert!(!value.turns[0].counts_complete && !value.turns[0].references_complete);
    let mut context = prompt();
    context.metadata["input_origin"] = json!("context_injection");
    assert!(present(&[context, tool(2, false)], &BTreeSet::new(), true).is_none());
    let mut generated = prompt();
    generated.provider = "unknown".into();
    assert!(present(&[generated, tool(2, false)], &BTreeSet::new(), true).is_none());
}

#[test]
fn interruption_retains_partial_output_but_cannot_claim_success_or_an_answer() {
    let mut result = tool(3, true);
    result.status = Some(AgentChatStatus::Processing);
    result.text = Some("Synthetic partial output".into());
    let page = present(
        &[prompt(), tool(2, false), result, answer()],
        &BTreeSet::new(),
        false,
    )
    .unwrap();
    assert_eq!(page.turns[0].status, ChatTurnStatus::Incomplete);
    assert!(page.turns[0].prefix_attributed);
    assert!(!page.turns[0].membership_complete && !page.turns[0].answer_recorded);
    assert_eq!(
        page.items[1].step.as_ref().unwrap().status,
        ChatStepStatus::Incomplete
    );
    assert_eq!(
        page.items[1].preview.as_ref().unwrap().text,
        "Synthetic partial output"
    );
    assert_eq!(page.items[2].kind, ChatPresentedItemKind::Narration);
}

#[test]
fn source_terminal_and_adapter_complete_are_both_required() {
    for invalid in ["no-complete", "idle", "wrong-terminal"] {
        let mut terminal = closure();
        match invalid {
            "no-complete" => terminal.metadata["chat_turn_membership_complete"] = json!(false),
            "idle" => terminal.status = Some(AgentChatStatus::Idle),
            _ => terminal.metadata["raw_type"] = json!("tool_result"),
        }
        let page = present(&[prompt(), answer(), terminal], &BTreeSet::new(), false).unwrap();
        assert_eq!(page.turns[0].status, ChatTurnStatus::Incomplete);
        assert!(!page.turns[0].membership_complete && !page.turns[0].answer_recorded);
    }
}

#[test]
fn claude_end_turn_or_result_can_close_the_same_request() {
    let mut events = vec![prompt(), answer()];
    for event in &mut events {
        event.provider = "claude".into();
        event.metadata["chat_turn_adapter"] = json!("claude_parent_chain_v1");
    }
    events[1].metadata["chat_turn_membership_complete"] = json!(true);
    let page = present(&events, &BTreeSet::new(), false).unwrap();
    assert!(page.turns[0].membership_complete && page.turns[0].answer_recorded);
    events[1].metadata["chat_turn_membership_complete"] = json!(false);
    let mut result = closure();
    result.provider = "claude".into();
    result.metadata["chat_turn_adapter"] = json!("claude_parent_chain_v1");
    result.metadata["raw_type"] = json!("result");
    result.status = Some(AgentChatStatus::Failed);
    events.push(result);
    let page = present(&events, &BTreeSet::new(), false).unwrap();
    assert!(page.turns[0].membership_complete);
    assert_eq!(page.turns[0].status, ChatTurnStatus::Failed);
}

#[test]
fn unsupported_foreign_and_unqualified_identities_fail_closed() {
    for field in [
        "provider",
        "adapter",
        "agent",
        "epoch",
        "physical",
        "coordinates",
        "provider-turn",
        "root",
    ] {
        let mut middle = tool(2, false);
        match field {
            "provider" => middle.provider = "gemini".into(),
            "adapter" => middle.metadata["chat_turn_adapter"] = json!("claude_parent_chain_v1"),
            "agent" => middle.session_id = "foreign-agent".into(),
            "epoch" => middle.metadata["chat_source_epoch"] = json!("foreign-epoch"),
            "physical" => middle.metadata["chat_source_ref"] = json!(null),
            "coordinates" => {
                middle.metadata["chat_source_end"] = middle.metadata["chat_source_start"].clone()
            }
            "provider-turn" => middle.metadata["provider_turn_id"] = json!("other-provider-turn"),
            _ => middle.metadata["request_root_id"] = json!("unopened-root"),
        }
        assert!(
            present(&[prompt(), middle, answer()], &BTreeSet::new(), true).is_none(),
            "{field}"
        );
    }
    assert!(preview(
        &Scope {
            source_key: None,
            ..scope()
        },
        &[prompt()],
        "admission",
        &BTreeSet::new(),
        true,
        &PreviewOwners::default()
    )
    .unwrap()
    .is_none());
    assert!(preview(
        &scope(),
        &[prompt()],
        "",
        &BTreeSet::new(),
        true,
        &PreviewOwners::default()
    )
    .unwrap()
    .is_none());
}

#[test]
fn mixed_rows_keep_all_refs_and_no_detail_authority_and_round_trip_wire_validation() {
    let mut events = vec![prompt(), tool(2, false), tool(3, true), answer(), closure()];
    events[0].metadata["conversation_archive_id"] = json!(scope().conversation_id);
    for event in &mut events[1..] {
        event.metadata["chat_provisional"] = json!(true);
        event.metadata["chat_detail_ref"] = json!("untrusted:0");
        event.metadata["chat_step_detail_ref"] = json!("untrusted:tail");
    }
    events[2].text = Some("🦆".repeat(400));
    let page = present(&events, &BTreeSet::new(), false).unwrap();
    assert!(page.preview_bridge.is_none() && page.detail.is_none());
    let refs = page
        .items
        .iter()
        .flat_map(|item| item.event_refs.iter())
        .chain(
            page.state_event_refs
                .iter()
                .map(|reference| &reference.event_id),
        )
        .collect::<BTreeSet<_>>();
    assert_eq!(refs, events.iter().map(|event| &event.id).collect());
    for item in &page.items {
        assert_eq!(item.provenance, ChatItemProvenance::Provisional);
        assert_eq!(item.admission_id.as_deref(), Some("synthetic-admission"));
        assert!(item.preview.as_ref().unwrap().text.len() <= 1024);
        if let Some(step) = &item.step {
            assert!(
                step.detail_ref.is_none()
                    && step.invocation_ref.is_none()
                    && step.result_ref.is_none()
                    && step.approval_ref.is_none()
            );
            assert_eq!(step.detail_availability, ChatDetailAvailability::Pending);
        }
    }
    let envelope = ChatPresentation {
        version: 1,
        payload: serde_json::to_value(&page).unwrap(),
    };
    let bytes = serde_json::to_vec(&envelope).unwrap();
    assert!(bytes.len() <= CHAT_PRESENTATION_MAX_BYTES);
    let decoded: ChatPresentation = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded.decode_v1().unwrap(), Some(page));
    assert!(decoded.payload["turns"][0]["prefix_attributed"]
        .as_bool()
        .unwrap());
}

#[test]
fn caller_order_is_reused_and_budget_or_duplicate_rows_reject_the_entire_slice() {
    let events = [prompt(), tool(2, false), tool(3, true)];
    let forward = present(&events, &BTreeSet::new(), true).unwrap();
    let mut reverse = events.to_vec();
    reverse.reverse();
    assert_eq!(forward, present(&reverse, &BTreeSet::new(), true).unwrap());
    assert_eq!(
        forward.items[0].order_key,
        format!(
            "{}:{}",
            super::super::super::chat_read::presentation_order(&events[0]).unwrap(),
            forward.items[0].item_key
        )
    );
    assert!(present(&[], &BTreeSet::new(), true).is_none());
    assert!(present(&[prompt(), prompt()], &BTreeSet::new(), true).is_none());
    assert!(present(
        &vec![prompt(); CHAT_PRESENTATION_MAX_ITEMS + 1],
        &BTreeSet::new(),
        true
    )
    .is_none());
}

#[test]
fn persisted_opening_certification_is_head_bound_and_excludes_invalid_or_unsupported_turns() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    let events = [prompt(), tool(2, false), tool(3, true)];
    for event in &events {
        let order = super::super::super::chat_read::presentation_order(event).unwrap();
        let observed = index
            .observe(&mut writer, &scope(), event, &order, None)
            .unwrap()
            .unwrap();
        let kind = item_kind(event).unwrap();
        assert_eq!(
            observed,
            identity(&scope(), event, "request-root", &kind, None)
                .unwrap()
                .unwrap()
                .item_key
        );
    }
    let original = index.clone();
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &events[1..],
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let known = owners.proven_openings().clone();
    assert_eq!(known, BTreeSet::from(["request-root".into()]));
    let serialized = serde_json::to_value(&owners).unwrap();
    assert!(serialized.get("opening_roots").is_none());
    let restored_owners: PreviewOwners = serde_json::from_value(serialized).unwrap();
    assert!(restored_owners.proven_openings().is_empty());
    assert!(preview(
        &scope(),
        &events[1..],
        "synthetic-admission",
        owners.proven_openings(),
        true,
        &owners
    )
    .unwrap()
    .is_some());
    let mut forged = events[1].clone();
    forged.metadata["request_root_id"] = json!("forged-root");
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &[forged],
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    let mut partial = tool(6, true);
    partial.metadata["chat_turn_membership_partial"] = json!(true);
    index
        .observe(&mut writer, &scope(), &partial, "source:0006", None)
        .unwrap();
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &events[1..],
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    assert_eq!(
        original
            .preview_owners_reusing_nodes(
                &mut Store::new(temp.path()),
                &scope(),
                &events[1..],
                None,
                &mut SeekCache::default(),
            )
            .unwrap()
            .unwrap()
            .proven_openings(),
        &known
    );
    let mut unsupported = event(7, AgentChatEventKind::Message);
    unsupported.role = Some(AgentChatRole::System);
    let mut other = original.clone();
    other
        .observe(&mut writer, &scope(), &unsupported, "source:0007", None)
        .unwrap();
    assert!(other
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &events[1..],
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    let mut inactive = Index::default();
    inactive
        .observe(&mut writer, &scope(), &events[1], "source:0002", None)
        .unwrap();
    assert!(inactive
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &events[1..2],
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    assert!(original
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &vec![events[1].clone(); CHAT_PRESENTATION_MAX_ITEMS + 1],
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .is_none());
}

fn source_headers(provider: &str, lines: &[&str]) -> Vec<AgentChatEvent> {
    use crate::providers::chat_transcript::{
        normalize_chat_lines_with_state, TranscriptNormalizationState,
    };
    let mut state = TranscriptNormalizationState::default();
    let mut start = 0u64;
    let mut headers = Vec::new();
    for line in lines {
        let end = start + line.len() as u64 + 1;
        let events = normalize_chat_lines_with_state(
            scope().agent_id,
            provider,
            [line],
            &mut state,
            true,
            false,
        )
        .unwrap();
        for mut event in events {
            event.id = format!("synthetic-native:{start}:{end}");
            event.metadata["chat_source_ref"] = json!(event.id);
            event.metadata["chat_source_start"] = json!(start);
            event.metadata["chat_source_end"] = json!(end);
            event.metadata["chat_source_epoch"] = json!(scope().source_epoch);
            headers.push(super::super::super::chat_read::header(&event));
        }
        start = end;
    }
    headers
}

#[test]
fn normalizer_source_headers_prove_running_reachability_and_expose_unrooted_start_boundary() {
    let session = r#"{"type":"session_meta","payload":{"id":"synthetic-session"}}"#;
    let context = r#"{"type":"turn_context","payload":{"turn_id":"synthetic-turn"}}"#;
    let request = r#"{"type":"response_item","payload":{"type":"message","id":"synthetic-request","role":"user","content":[{"type":"input_text","text":"Run synthetic tests."}],"internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-turn","content_item_kinds":["user.text"]}}}"#;
    let started =
        r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"synthetic-turn"}}"#;
    let call = r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"synthetic-invocation","turn_id":"synthetic-turn","arguments":"{\"cmd\":\"echo synthetic\",\"description\":\"Run synthetic tests\"}"}}"#;
    let result = r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"synthetic-invocation","turn_id":"synthetic-turn","output":"partial synthetic output"}}"#;
    let headers = source_headers("codex", &[session, context, request, started, call, result]);
    assert_eq!(headers.len(), 4);
    assert!(headers
        .iter()
        .all(|event| string(event, "request_root_id").is_some()));
    let page = present(&headers, &BTreeSet::new(), true)
        .expect("prompt before start proves the whole prefix");
    assert_eq!(page.turns[0].status, ChatTurnStatus::Running);
    assert_eq!(page.turns[0].step_count, 1);
    let step = page
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Step)
        .unwrap();
    assert_eq!(step.event_refs.len(), 2);
    let source = key(&(
        "turn-source-v1",
        key(&("turn-owner-v1", scope().agent_id, scope().conversation_id)).unwrap(),
        scope().source_key,
        scope().source_epoch,
    ))
    .unwrap();
    let turn_key = key(&(
        "turn-v1",
        source,
        string(&headers[0], "request_root_id").unwrap(),
    ))
    .unwrap();
    assert_eq!(
        step.item_key,
        key(&("step-v1", turn_key, "synthetic-invocation")).unwrap()
    );
    assert_eq!(page.state_event_refs.len(), 1);

    // The native timing fixture starts the task before its prompt. That state
    // row is physically admitted but cannot acquire request membership later.
    let early_start = source_headers("codex", &[session, started, context, request, call, result]);
    assert_eq!(early_start[0].metadata["raw_type"], "task_started");
    assert!(string(&early_start[0], "request_root_id").is_none());
    assert!(present(&early_start, &BTreeSet::new(), true).is_none());
    // A caller-selected bounded suffix is independently attributable; the
    // builder never removes the unrooted middle/start row from its input.
    assert!(present(&early_start[1..], &BTreeSet::new(), true).is_some());
}

#[test]
fn claude_normalizer_source_headers_pair_invocation_and_partial_result() {
    let lines = [
        r#"{"type":"user","uuid":"request","sessionId":"synthetic-session","parentUuid":null,"message":{"role":"user","content":"Run synthetic tests."}}"#,
        r#"{"type":"assistant","uuid":"call","sessionId":"synthetic-session","parentUuid":"request","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"echo synthetic"}}],"stop_reason":"tool_use"}}"#,
        r#"{"type":"user","uuid":"result","sessionId":"synthetic-session","parentUuid":"call","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"partial synthetic output"}]}}"#,
    ];
    let headers = source_headers("claude", &lines);
    assert_eq!(headers.len(), 3);
    let page = present(&headers, &BTreeSet::new(), true).unwrap();
    assert_eq!(page.turns[0].status, ChatTurnStatus::Running);
    assert_eq!(page.turns[0].step_count, 1);
    let step = page
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Step)
        .unwrap();
    assert_eq!(step.event_refs.len(), 2);
    assert_eq!(
        step.preview.as_ref().unwrap().text,
        "partial synthetic output"
    );
}

fn normalized_mirror() -> Vec<AgentChatEvent> {
    source_headers(
        "codex",
        &[
            r#"{"type":"session_meta","payload":{"id":"synthetic-session"}}"#,
            r#"{"type":"turn_context","payload":{"turn_id":"synthetic-turn"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","id":"synthetic-request","role":"user","content":[{"type":"input_text","text":"Run synthetic tests."}],"internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-turn","content_item_kinds":["user.text"]}}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"synthetic-turn"}}"#,
            r#"{"type":"event_msg","payload":{"type":"agent_message","turn_id":"synthetic-turn","message":"Synthetic mirrored answer."}}"#,
            r#"{"type":"response_item","payload":{"type":"message","id":"synthetic-answer","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Synthetic mirrored answer."}],"internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-turn"}}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"synthetic-turn"}}"#,
        ],
    )
}

fn observe_rows(index: &mut Index, store: &mut Store, events: &[AgentChatEvent]) {
    for event in events {
        let order = super::super::super::chat_read::presentation_order(event).unwrap();
        assert!(index
            .observe(store, &scope(), event, &order, None)
            .unwrap()
            .is_some());
    }
}

fn merged_mirror(events: &[AgentChatEvent]) -> AgentChatEvent {
    let mut displayed = events[3].clone();
    displayed.id = "display:synthetic-mirror".into();
    displayed.metadata["chat_display_member_ids"] = json!([events[2].id, events[3].id]);
    displayed.metadata["chat_display_physical_id"] = json!(events[3].id);
    displayed
}

#[test]
fn normalized_mirror_keeps_canonical_p1_owner_and_order_when_p2_displays_and_closes() {
    let events = normalized_mirror();
    assert_eq!(events.len(), 5);
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    observe_rows(&mut index, &mut writer, &events[..4]);
    let displayed = merged_mirror(&events);
    assert_eq!(displayed.metadata["chat_source_ref"], events[3].id);
    assert_ne!(events[2].id, events[3].id);
    let root = string(&events[0], "request_root_id").unwrap();
    let expected = identity(
        &scope(),
        &events[2],
        root,
        &ChatPresentedItemKind::Narration,
        None,
    )
    .unwrap()
    .unwrap();
    let naive = identity(
        &scope(),
        &displayed,
        root,
        &ChatPresentedItemKind::Answer,
        None,
    )
    .unwrap()
    .unwrap();
    assert_ne!(
        naive.item_key, expected.item_key,
        "fixture must expose representative/owner mismatch"
    );
    let mut running_events = vec![events[0].clone(), events[1].clone(), displayed.clone()];
    for event in &mut running_events {
        // Bounded SourceSeed uses byte offsets as sequence; canonical ingest
        // used the normalizer's record sequence when persisting these items.
        event.sequence = event.metadata["chat_source_start"].as_u64();
        event.metadata["chat_provisional"] = json!(true);
    }
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &running_events,
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let running = preview(
        &scope(),
        &running_events,
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &owners,
    )
    .unwrap()
    .unwrap();
    let before = running
        .items
        .iter()
        .find(|item| item.event_refs.contains(&displayed.id))
        .unwrap();
    assert_eq!(before.item_key, expected.item_key);
    assert_eq!(before.kind, ChatPresentedItemKind::Narration);
    assert!(
        matches!(&before.evidence, ChatEvidenceIdentity::Source { record_key, .. } if record_key == &events[2].id)
    );
    assert!(!running.turns[0].membership_complete && !running.turns[0].answer_recorded);

    observe_rows(&mut index, &mut writer, &events[4..]);
    let canonical_events = vec![events[0].clone(), events[1].clone(), displayed.clone()];
    let canonical = index
        .page(&mut Store::new(temp.path()), &canonical_events, true)
        .unwrap()
        .unwrap();
    let after = canonical
        .items
        .iter()
        .find(|item| item.event_refs.contains(&displayed.id))
        .unwrap();
    assert_eq!(
        (&before.item_key, &before.order_key, &before.evidence),
        (&after.item_key, &after.order_key, &after.evidence)
    );
    assert_eq!(after.kind, ChatPresentedItemKind::Answer);
    assert!(canonical.turns[0].membership_complete);
    assert_eq!(running.turns[0].turn_key, canonical.turns[0].turn_key);
    assert_eq!(running.turns[0].order_key, canonical.turns[0].order_key);
    for item in &running.items {
        let canonical_item = canonical
            .items
            .iter()
            .find(|canonical| canonical.item_key == item.item_key)
            .unwrap();
        assert_eq!(item.order_key, canonical_item.order_key);
    }

    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &canonical_events,
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let mut closed_events = canonical_events;
    closed_events.push(events[4].clone());
    let closed = preview(
        &scope(),
        &closed_events,
        "synthetic-admission",
        &BTreeSet::new(),
        false,
        &owners,
    )
    .unwrap()
    .unwrap();
    assert!(closed.turns[0].membership_complete && closed.turns[0].answer_recorded);
    let closed_answer = closed
        .items
        .iter()
        .find(|item| item.item_key == expected.item_key)
        .unwrap();
    assert_eq!(closed_answer.kind, ChatPresentedItemKind::Answer);
    assert_eq!(closed_answer.order_key, before.order_key);
}

#[test]
fn merged_preview_rejects_any_unresolved_member_and_missing_or_changed_physical_evidence() {
    let events = normalized_mirror();
    let displayed = merged_mirror(&events);
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut partial = Index::default();
    observe_rows(&mut partial, &mut writer, &events[..2]);
    observe_rows(&mut partial, &mut writer, &events[3..4]);
    let page = vec![events[0].clone(), displayed.clone()];
    assert!(partial
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &page,
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    assert!(preview(
        &scope(),
        &page,
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &PreviewOwners::default()
    )
    .unwrap()
    .is_none());

    let mut complete = Index::default();
    observe_rows(&mut complete, &mut writer, &events[..4]);
    let owners = complete
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &page,
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let mut changed = page.clone();
    changed[1].metadata["chat_display_member_ids"] = json!(["unresolved-physical", events[3].id]);
    assert!(complete
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &changed,
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    assert!(preview(
        &scope(),
        &changed,
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &owners
    )
    .unwrap()
    .is_none());
    for field in ["chat_source_ref", "chat_display_member_ids"] {
        let mut missing = page.clone();
        missing[1].metadata.as_object_mut().unwrap().remove(field);
        assert!(
            complete
                .preview_owners_reusing_nodes(
                    &mut Store::new(temp.path()),
                    &scope(),
                    &missing,
                    None,
                    &mut SeekCache::default(),
                )
                .unwrap()
                .is_none(),
            "{field}"
        );
    }
    let mut foreign = scope();
    foreign.source_epoch = Some("foreign-epoch");
    assert!(preview(
        &foreign,
        &page,
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &owners
    )
    .unwrap()
    .is_none());
}

#[test]
fn uncaptured_single_physical_rows_derive_keys_and_existing_calls_keep_their_persisted_order() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    let request = prompt();
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            std::slice::from_ref(&request),
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let initial = preview(
        &scope(),
        std::slice::from_ref(&request),
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &owners,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        initial.items[0].item_key,
        identity(
            &scope(),
            &request,
            "request-root",
            &ChatPresentedItemKind::Prompt,
            None
        )
        .unwrap()
        .unwrap()
        .item_key
    );
    let call = tool(2, false);
    observe_rows(&mut index, &mut writer, &[request.clone(), call.clone()]);
    let canonical = index
        .page(&mut Store::new(temp.path()), &[request, call.clone()], true)
        .unwrap()
        .unwrap();
    let mut result = tool(3, true);
    result.sequence = Some(30_000);
    result.metadata["chat_provisional"] = json!(true);
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            std::slice::from_ref(&result),
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let preview = preview(
        &scope(),
        &[result],
        "synthetic-admission",
        &BTreeSet::from(["request-root".into()]),
        true,
        &owners,
    )
    .unwrap()
    .unwrap();
    let expected = canonical
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Step)
        .unwrap();
    assert_eq!(
        (&preview.items[0].item_key, &preview.items[0].order_key),
        (&expected.item_key, &expected.order_key)
    );
    assert_eq!(
        preview.items[0].preview.as_ref().unwrap().text,
        "Synthetic content 3"
    );
}

#[test]
fn certified_unpaired_result_order_survives_capture_with_a_later_preferred_call_order() {
    let prefix_events = [
        prompt(),
        tool(2, true),
        event(3, AgentChatEventKind::Message),
    ];
    let prefix = present(&prefix_events, &BTreeSet::new(), true).unwrap();
    let step = prefix
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Step)
        .unwrap();
    let step_key = step.item_key.clone();
    let prefix_order = step.order_key.clone();
    let previous = PreviewOwners::from_presentation(&scope(), "synthetic-admission", &prefix)
        .unwrap()
        .unwrap();
    let call = tool(4, false);
    let preferred = super::super::super::chat_read::presentation_order(&call).unwrap();
    let events = [prefix_events.to_vec(), vec![call, closure()]].concat();
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    for event in &events {
        let order = super::super::super::chat_read::presentation_order(event).unwrap();
        let preferred =
            (item_kind(event) == Some(ChatPresentedItemKind::Step)).then_some(preferred.as_str());
        index
            .observe(&mut writer, &scope(), event, &order, preferred)
            .unwrap()
            .unwrap();
    }
    let canonical = index
        .page(&mut Store::new(temp.path()), &events, true)
        .unwrap()
        .unwrap();
    let captured_step = canonical
        .items
        .iter()
        .find(|item| item.item_key == step_key)
        .unwrap();
    assert_ne!(captured_step.order_key, prefix_order);
    let narration = canonical
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Narration)
        .unwrap();
    assert!(prefix_order < narration.order_key);
    assert!(captured_step.order_key > narration.order_key);
    assert!(canonical.turns[0].membership_complete);

    let mut pinned = canonical.clone();
    previous.preserve_item_orders(&mut pinned);
    let mut expected = canonical.clone();
    expected
        .items
        .iter_mut()
        .find(|item| item.item_key == step_key)
        .unwrap()
        .order_key
        .clone_from(&prefix_order);
    assert_eq!(pinned, expected);
    pinned.items.sort_by(|left, right| {
        left.order_key
            .cmp(&right.order_key)
            .then(left.item_key.cmp(&right.item_key))
    });
    pinned.validate().unwrap();
    assert!(
        pinned
            .items
            .iter()
            .position(|item| item.item_key == step_key)
            .unwrap()
            < pinned
                .items
                .iter()
                .position(|item| item.kind == ChatPresentedItemKind::Narration)
                .unwrap()
    );
    assert_eq!(
        index
            .page(&mut Store::new(temp.path()), &events, true)
            .unwrap()
            .unwrap(),
        canonical
    );
    for carried in [
        PreviewOwners::from_presentation(&scope(), "synthetic-admission", &pinned)
            .unwrap()
            .unwrap(),
        previous
            .remember(&scope(), "synthetic-admission", &pinned)
            .unwrap()
            .unwrap(),
    ] {
        let mut second_poll = index
            .page(&mut Store::new(temp.path()), &events, true)
            .unwrap()
            .unwrap();
        assert_eq!(second_poll, canonical);
        carried.preserve_item_orders(&mut second_poll);
        let remembered = carried
            .remember(&scope(), "synthetic-admission", &second_poll)
            .unwrap()
            .unwrap();
        assert_eq!(remembered.remembered[&step_key].order_key, prefix_order);
        second_poll.items.sort_by(|left, right| {
            left.order_key
                .cmp(&right.order_key)
                .then(left.item_key.cmp(&right.item_key))
        });
        second_poll.validate().unwrap();
        assert_eq!(second_poll, pinned);
    }
    let fresh = PreviewOwners::from_presentation(&scope(), "admission-b", &canonical)
        .unwrap()
        .unwrap();
    let mut fresh_view = canonical.clone();
    fresh.preserve_item_orders(&mut fresh_view);
    assert_eq!(fresh_view, canonical);

    for mismatch in ["item", "turn", "source", "epoch"] {
        let mut other = canonical.clone();
        let item = other
            .items
            .iter_mut()
            .find(|item| item.item_key == step_key)
            .unwrap();
        match mismatch {
            "item" => item.item_key = "another-item".into(),
            "turn" => item.turn_key = "another-turn".into(),
            "source" | "epoch" => {
                let ChatEvidenceIdentity::Source {
                    source_key,
                    source_epoch,
                    ..
                } = &mut item.evidence
                else {
                    panic!("source-backed step");
                };
                if mismatch == "source" {
                    *source_key = "another-source".into();
                } else {
                    *source_epoch = "another-epoch".into();
                }
            }
            _ => unreachable!(),
        }
        let expected = other.clone();
        previous.preserve_item_orders(&mut other);
        assert_eq!(other, expected, "{mismatch}");
    }
}

#[test]
fn full_normalized_running_window_reuses_observations_without_rereading_values() {
    let mut lines = vec![
        json!({"type":"session_meta","payload":{"id":"capacity-session"}}),
        json!({"type":"turn_context","payload":{"turn_id":"capacity-turn"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"capacity-turn"}}),
        json!({"type":"response_item","payload":{"type":"message","id":"capacity-prompt","role":"user","content":[{"type":"input_text","text":"Run synthetic tests."}],"internal_chat_message_metadata_passthrough":{"turn_id":"capacity-turn","content_item_kinds":["user.text"]}}}),
    ];
    for ordinal in 0..100 {
        lines.push(json!({"type":"event_msg","payload":{"type":"agent_message","turn_id":"capacity-turn","message":format!("Synthetic progress {ordinal}")}}));
    }
    let lines: Vec<_> = lines.iter().map(ToString::to_string).collect();
    let epoch = digest(b"synthetic-native-capacity-epoch");
    let native_scope = Scope {
        source_epoch: Some(&epoch),
        ..scope()
    };
    let mut offset = 0u64;
    let line_hashes: BTreeMap<_, _> = lines
        .iter()
        .map(|line| {
            let start = offset;
            offset += line.len() as u64 + 1;
            (start, digest(line.as_bytes()))
        })
        .collect();
    let inputs: Vec<_> = lines.iter().map(String::as_str).collect();
    let mut events: Vec<_> = source_headers("codex", &inputs)
        .into_iter()
        .filter(|event| string(event, "request_root_id").is_some())
        .collect();
    let temp = tempfile::tempdir().unwrap();
    for event in &mut events {
        let start = event.metadata["chat_source_start"].as_u64().unwrap();
        let physical = format!(
            "source:{}:{epoch}:{start}:{}:0",
            native_scope.agent_id, line_hashes[&start]
        );
        event.id.clone_from(&physical);
        event.metadata["chat_source_ref"] = json!(physical);
        event.metadata["chat_source_epoch"] = json!(epoch);
    }
    let mut index = Index::default();
    // Capture setup uses separate bounded checkpoints; each measured read below
    // keeps one unchanged Store budget across the entire 80-row window.
    for checkpoint in events.chunks(8) {
        let mut writer = Store::writer(temp.path());
        for event in checkpoint {
            let order = super::super::super::chat_read::presentation_order(event).unwrap();
            assert!(index
                .observe(&mut writer, &native_scope, event, &order, None)
                .unwrap()
                .is_some());
        }
    }
    let window = &events[events.len() - CHAT_PRESENTATION_MAX_ITEMS..];
    assert_eq!(window.len(), 80);
    assert!(window.iter().all(
        |event| event.metadata.get("chat_display_member_ids").is_none()
            && event.metadata.get("chat_turn_key").is_none()
    ));

    let mut once = Store::new(temp.path());
    let mut repeated = Store::new(temp.path());
    let mut once_cache = SeekCache::default();
    let mut repeated_cache = SeekCache::default();
    for event in window {
        let resolved = index
            .page_members(&mut once, event, &mut once_cache)
            .unwrap()
            .unwrap();
        let old = index
            .page_members(&mut repeated, event, &mut repeated_cache)
            .unwrap()
            .unwrap();
        let duplicate: Observation = lookup(
            &mut repeated,
            &index.events,
            &digest(event.id.as_bytes()),
            &mut repeated_cache,
        )
        .unwrap()
        .unwrap();
        assert!(resolved.members == old.members);
        assert!(resolved.members == vec![duplicate.membership]);
        assert_eq!(
            resolved.source_order.as_deref(),
            Some(duplicate.source_order.as_str())
        );
    }
    assert_eq!(
        repeated.objects, once.objects,
        "a second typed view must reuse the hash-validated observation bytes"
    );
    assert_eq!(repeated.bytes, once.bytes);

    let mut ordinary_store = Store::new(temp.path());
    let ordinary = index
        .preview_owners_reusing_nodes(
            &mut ordinary_store,
            &native_scope,
            window,
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let mut inline = window.to_vec();
    for event in &mut inline {
        let Identity {
            turn_key, item_key, ..
        } = identity(
            &native_scope,
            event,
            string(event, "request_root_id").unwrap(),
            &item_kind(event).unwrap(),
            None,
        )
        .unwrap()
        .unwrap();
        event.metadata["chat_turn_key"] = json!(turn_key);
        event.metadata["chat_turn_item_key"] = json!(item_key);
    }
    let mut inline_store = Store::new(temp.path());
    let control = index
        .preview_owners_reusing_nodes(
            &mut inline_store,
            &native_scope,
            &inline,
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(ordinary_store.objects, inline_store.objects);
    assert_eq!(ordinary_store.bytes, inline_store.bytes);
    assert_eq!(
        serde_json::to_value(&ordinary).unwrap(),
        serde_json::to_value(&control).unwrap()
    );
    assert_eq!(ordinary.proven_openings(), control.proven_openings());
    let mut authenticated = inline.clone();
    for event in &mut authenticated {
        event.metadata["_wardian_canonical_row"] = json!(true);
        event.sequence = event.sequence.map(|sequence| sequence + 100_000);
    }
    let mut authenticated_store = Store::new(temp.path());
    let native = index
        .preview_owners_reusing_nodes(
            &mut authenticated_store,
            &native_scope,
            &authenticated,
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        inline_store.objects - authenticated_store.objects,
        once.objects
    );
    assert_eq!(inline_store.bytes - authenticated_store.bytes, once.bytes);
    assert!(inline_store.objects - authenticated_store.objects >= 80);
    assert_eq!(
        serde_json::to_value(&native).unwrap(),
        serde_json::to_value(&control).unwrap()
    );
    for event in &authenticated {
        assert_eq!(
            native.rows[&event.id].source_order,
            super::super::super::chat_read::presentation_order(event).unwrap()
        );
    }
    let mut altered = authenticated[0].clone();
    altered.metadata["chat_source_end"] =
        json!(altered.metadata["chat_source_end"].as_u64().unwrap() + 1);
    let fallback = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &native_scope,
            std::slice::from_ref(&altered),
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        fallback.rows[&altered.id].source_order,
        control.rows[&altered.id].source_order
    );
    assert_ne!(
        fallback.rows[&altered.id].source_order,
        super::super::super::chat_read::presentation_order(&altered).unwrap()
    );
    let foreign_epoch = digest(b"another-native-source-epoch");
    let foreign = Scope {
        source_epoch: Some(&foreign_epoch),
        ..scope()
    };
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &foreign,
            &authenticated,
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    let mut forged = authenticated[0].clone();
    forged.metadata["chat_turn_item_key"] = json!("f".repeat(64));
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &native_scope,
            &[forged],
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());

    let value = preview(
        &native_scope,
        window,
        "capacity-admission",
        ordinary.proven_openings(),
        true,
        &ordinary,
    )
    .unwrap()
    .unwrap();
    assert_eq!(value.items.len(), 80);
    assert_eq!(value.turns[0].status, ChatTurnStatus::Running);
    assert!(!value.turns[0].membership_complete);
}

#[test]
fn alias_and_inline_members_keep_their_existing_source_order_fallback() {
    let events = normalized_mirror();
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    observe_rows(&mut index, &mut writer, &events[..4]);
    let merged = merged_mirror(&events);
    let derived = identity(
        &scope(),
        &events[3],
        string(&events[3], "request_root_id").unwrap(),
        &item_kind(&events[3]).unwrap(),
        None,
    )
    .unwrap()
    .unwrap();
    let mut inline = events[3].clone();
    inline.metadata["chat_turn_key"] = json!(derived.turn_key);
    inline.metadata["chat_turn_item_key"] = json!(derived.item_key);
    let expected = super::super::super::chat_read::presentation_order(&events[3]).unwrap();
    for event in [merged, inline] {
        let resolved = index
            .page_members(
                &mut Store::new(temp.path()),
                &event,
                &mut SeekCache::default(),
            )
            .unwrap()
            .unwrap();
        assert!(resolved.source_order.is_none());
        let owners = index
            .preview_owners_reusing_nodes(
                &mut Store::new(temp.path()),
                &scope(),
                std::slice::from_ref(&event),
                None,
                &mut SeekCache::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(owners.rows[&event.id].source_order, expected);
    }
}

#[test]
fn persisted_single_owner_order_survives_source_seed_sequence_reset() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    let events = [prompt(), event(4, AgentChatEventKind::Message)];
    for event in &events {
        // Retain the real pre-PR3 source-order recipe in an existing Head even
        // after the caller's shared formatter adopts sequence-independent order.
        let order = format!(
            "source:{:020}:{:020}:{:020}:{}",
            event.metadata["chat_source_start"].as_u64().unwrap(),
            event.metadata["chat_source_end"].as_u64().unwrap(),
            event.sequence.unwrap(),
            event.id
        );
        index
            .observe(&mut writer, &scope(), event, &order, None)
            .unwrap()
            .unwrap();
    }
    let canonical = index
        .page(&mut Store::new(temp.path()), &events, true)
        .unwrap()
        .unwrap();
    let mut seeded = events.clone();
    for event in &mut seeded {
        event.sequence = event.metadata["chat_source_start"].as_u64();
        event.metadata["chat_provisional"] = json!(true);
    }
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &seeded,
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let preview = preview(
        &scope(),
        &seeded,
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &owners,
    )
    .unwrap()
    .unwrap();
    assert_eq!(preview.turns[0].order_key, canonical.turns[0].order_key);
    for (before, after) in preview.items.iter().zip(&canonical.items) {
        assert_eq!(
            (&before.item_key, &before.order_key),
            (&after.item_key, &after.order_key)
        );
    }
}

#[test]
fn remembered_owner_preserves_a_running_turn_when_its_new_mirror_is_uncaptured() {
    let events = normalized_mirror();
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    observe_rows(&mut index, &mut writer, &events[..3]);
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &events[..3],
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let before = preview(
        &scope(),
        &events[..3],
        "admission-a",
        &BTreeSet::new(),
        true,
        &owners,
    )
    .unwrap()
    .unwrap();
    let saved = owners
        .remember(&scope(), "admission-a", &before)
        .unwrap()
        .unwrap();
    let bytes = serde_json::to_vec(&saved).unwrap();
    let json = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        !json.contains("Synthetic mirrored answer.")
            && !json.contains("preview")
            && !json.contains("detail_ref")
            && !json.contains("invocation_ref")
    );
    assert!(saved.rows.is_empty());
    assert!(saved.remembered.values().all(|item| item.step.is_none()));
    let saved: PreviewOwners = serde_json::from_slice(&bytes).unwrap();
    let displayed = merged_mirror(&events);
    let page = vec![events[0].clone(), events[1].clone(), displayed.clone()];
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &page,
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    let next = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &page,
            Some(&saved),
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let running = preview(
        &scope(),
        &page,
        "admission-a",
        &BTreeSet::new(),
        true,
        &next,
    )
    .unwrap()
    .unwrap();
    let p1 = before
        .items
        .iter()
        .find(|item| item.event_refs.contains(&events[2].id))
        .unwrap();
    let p2 = running
        .items
        .iter()
        .find(|item| item.event_refs.contains(&displayed.id))
        .unwrap();
    assert_eq!(
        (&p1.item_key, &p1.order_key, &p1.evidence),
        (&p2.item_key, &p2.order_key, &p2.evidence)
    );
    assert_eq!(p2.kind, ChatPresentedItemKind::Narration);
    let remembered = next
        .remember(&scope(), "admission-a", &running)
        .unwrap()
        .unwrap();
    assert_eq!(remembered.members.get(&events[3].id), Some(&p1.item_key));
    assert!(remembered.remembered.len() <= running.items.len() + running.state_event_refs.len());
    assert!(remembered.members.len() <= 2 * running.items.len() + running.state_event_refs.len());
    assert!(preview(
        &scope(),
        &page,
        "admission-b",
        &BTreeSet::new(),
        true,
        &next
    )
    .unwrap()
    .is_none());
    assert!(
        PreviewOwners::from_presentation(&scope(), "admission-b", &running)
            .unwrap()
            .is_none()
    );

    observe_rows(&mut index, &mut writer, &events[3..]);
    let closed = index
        .page(&mut Store::new(temp.path()), &page, true)
        .unwrap()
        .unwrap();
    let answer = closed
        .items
        .iter()
        .find(|item| item.item_key == p1.item_key)
        .unwrap();
    assert_eq!(answer.kind, ChatPresentedItemKind::Answer);
    assert_eq!(answer.order_key, p1.order_key);
}

#[test]
fn remembered_mirror_never_guesses_an_unknown_nonrepresentative_member() {
    let events = normalized_mirror();
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    observe_rows(&mut index, &mut writer, &events[..3]);
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &events[..3],
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let before = preview(
        &scope(),
        &events[..3],
        "admission-a",
        &BTreeSet::new(),
        true,
        &owners,
    )
    .unwrap()
    .unwrap();
    let saved = owners
        .remember(&scope(), "admission-a", &before)
        .unwrap()
        .unwrap();
    let mut displayed = merged_mirror(&events);
    displayed.metadata["chat_display_member_ids"] = json!([events[2].id, "unproven-member"]);
    displayed.metadata["chat_source_ref"] = json!(events[2].id);
    displayed.metadata["chat_display_physical_id"] = json!(events[2].id);
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &[displayed],
            Some(&saved),
            &mut SeekCache::default(),
        )
        .unwrap()
        .is_none());
    let mut foreign = scope();
    foreign.source_epoch = Some("another-epoch");
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &foreign,
            &events[..3],
            Some(&saved),
            &mut SeekCache::default(),
        )
        .unwrap()
        .is_none());
}

#[test]
fn owner_cache_remember_preserves_only_current_admitted_metadata() {
    let events = normalized_mirror();
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    observe_rows(&mut index, &mut writer, &events[..3]);
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &events[..3],
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let before = preview(
        &scope(),
        &events[..3],
        "admission-a",
        &BTreeSet::new(),
        true,
        &owners,
    )
    .unwrap()
    .unwrap();
    let cache = owners
        .remember(&scope(), "admission-a", &before)
        .unwrap()
        .unwrap();
    let message = before
        .items
        .iter()
        .find(|item| item.kind == ChatPresentedItemKind::Narration)
        .unwrap();
    let mut bounded = before.clone();
    bounded.state_event_refs.clear();
    bounded
        .items
        .retain(|item| item.item_key == message.item_key);
    for turn in &mut bounded.turns {
        turn.prompt_item_keys.clear();
        turn.answer_item_keys.clear();
        turn.references_complete = false;
    }
    let pruned = cache
        .remember(&scope(), "admission-a", &bounded)
        .unwrap()
        .unwrap();
    assert_eq!(pruned.remembered.len(), 1);
    assert!(pruned
        .members
        .values()
        .all(|item_key| item_key == &message.item_key));
}

#[test]
fn closed_turn_rejects_fresh_postterminal_physical_rows_and_preserves_replays() {
    let events = normalized_mirror();
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    observe_rows(&mut index, &mut writer, &events);
    let root = string(&events[0], "request_root_id").unwrap().to_owned();
    let roots = BTreeSet::from([root.clone(), "unproven-root".into()]);
    let closed = index
        .closed_roots_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &roots,
            &mut SeekCache::default(),
        )
        .unwrap();
    assert_eq!(closed.len(), 1);
    let cutoff = &closed[&root];
    assert_eq!(
        source_position(cutoff),
        Some((
            events[4].metadata["chat_source_start"].as_u64().unwrap(),
            events[4].metadata["chat_source_end"].as_u64().unwrap()
        ))
    );
    let snapshot = serde_json::to_vec(&index).unwrap();
    let mut rebound = events[0].clone();
    rebound.role = Some(AgentChatRole::Assistant);
    for alias in [false, true] {
        if alias {
            rebound.id = "contradictory-prompt-alias".into();
        }
        assert!(index
            .observe(&mut writer, &scope(), &rebound, "replayed-later", None)
            .is_err());
        assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
    }
    assert!(index
        .event_keys(&mut Store::new(temp.path()), &rebound.id)
        .unwrap()
        .is_none());
    let mut late = events[3].clone();
    late.id = "fresh-after-terminal".into();
    late.kind = AgentChatEventKind::ToolResult;
    late.role = Some(AgentChatRole::Tool);
    late.status = Some(AgentChatStatus::Failed);
    late.metadata["call_id"] = json!("late-call");
    late.metadata["chat_source_ref"] = json!(late.id);
    let start = events[4].metadata["chat_source_end"].as_u64().unwrap() + 1;
    late.metadata["chat_source_start"] = json!(start);
    late.metadata["chat_source_end"] = json!(start + 100);
    late.metadata["chat_turn_membership_partial"] = json!(true);
    let order = super::super::super::chat_read::presentation_order(&late).unwrap();
    assert!(Index::after_closed_source_order(&late, cutoff));
    assert!(index
        .observe(&mut writer, &scope(), &late, &order, None)
        .unwrap()
        .is_none());
    assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
    assert!(index
        .event_keys(&mut Store::new(temp.path()), &late.id)
        .unwrap()
        .is_none());
    let mut replay = events[2].clone();
    replay.metadata["chat_turn_membership_partial"] = json!(true);
    replay.metadata["provider_phase"] = json!("commentary");
    let expected = index
        .event_keys(&mut Store::new(temp.path()), &replay.id)
        .unwrap()
        .unwrap()
        .1;
    assert_eq!(
        index
            .observe(&mut writer, &scope(), &replay, "replayed-later", None)
            .unwrap(),
        Some(expected.clone())
    );
    assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
    replay.id = "new-projection-alias".into();
    let old_items = index.items.clone();
    let old_turns = index.turns.clone();
    assert_eq!(
        index
            .observe(&mut writer, &scope(), &replay, "replayed-later", None)
            .unwrap(),
        Some(expected.clone())
    );
    assert_eq!((&index.items, &index.turns), (&old_items, &old_turns));
    assert_eq!(
        index
            .event_keys(&mut Store::new(temp.path()), &replay.id)
            .unwrap()
            .unwrap()
            .1,
        expected
    );
    let mut closing = events[4].clone();
    closing.sequence = Some(u64::MAX);
    assert!(!Index::after_closed_source_order(&closing, cutoff));
    assert!(index
        .closed_roots_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            &BTreeSet::from_iter(
                (0..=CHAT_PRESENTATION_MAX_ITEMS).map(|index| format!("root-{index}"))
            ),
            &mut SeekCache::default(),
        )
        .is_err());
}

#[test]
fn cold_receipt_on_prompt_still_allows_physical_members_before_its_terminal_boundary() {
    let mut events = normalized_mirror();
    let receipt = json!({
        "version": 1, "source_key": scope().source_key, "source_epoch": scope().source_epoch,
        "provider": "codex", "adapter": "codex_bracket_v1",
        "provider_turn_id": events[0].metadata["provider_turn_id"],
        "request_root_id": events[0].metadata["request_root_id"],
        "source_ref": events[4].id, "start": events[4].metadata["chat_source_start"],
        "end": events[4].metadata["chat_source_end"], "sequence": events[4].sequence,
        "status": "succeeded", "complete": true, "partial": false,
    });
    events[0].metadata[super::super::super::chat_turn_closure::METADATA_KEY] = receipt;
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    observe_rows(&mut index, &mut writer, &events[..1]);
    let root = string(&events[0], "request_root_id").unwrap().to_owned();
    assert!(
        index
            .closed_roots_reusing_nodes(
                &mut Store::new(temp.path()),
                &scope(),
                &BTreeSet::from([root]),
                &mut SeekCache::default(),
            )
            .unwrap()
            .len()
            == 1
    );
    observe_rows(&mut index, &mut writer, &events[1..]);
    let displayed = merged_mirror(&events);
    let page = index
        .page(
            &mut Store::new(temp.path()),
            &[events[0].clone(), events[1].clone(), displayed],
            true,
        )
        .unwrap()
        .unwrap();
    assert!(page.turns[0].membership_complete && page.turns[0].answer_recorded);
    assert_eq!(page.turns[0].status, ChatTurnStatus::Completed);
    assert!(page
        .items
        .iter()
        .any(|item| item.kind == ChatPresentedItemKind::Answer));
}

#[test]
fn remembered_source_opening_is_not_retracted_by_missing_canonical_activation_metadata() {
    let events = normalized_mirror();
    let before = present(&events[..3], &BTreeSet::new(), true).unwrap();
    let seed = PreviewOwners::from_presentation(&scope(), "synthetic-admission", &before)
        .unwrap()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let inactive = Scope {
        can_activate: false,
        ..scope()
    };
    let mut index = Index::default();
    for event in &events[..3] {
        index
            .observe(
                &mut writer,
                &inactive,
                event,
                &super::super::super::chat_read::presentation_order(event).unwrap(),
                None,
            )
            .unwrap()
            .unwrap();
    }
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &inactive,
            &events[..3],
            None,
            &mut SeekCache::default()
        )
        .unwrap()
        .is_none());
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &inactive,
            &events[..3],
            Some(&seed),
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    assert!(preview(
        &inactive,
        &events[..3],
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &owners
    )
    .unwrap()
    .is_some());
    let saved = owners
        .remember(&inactive, "synthetic-admission", &before)
        .unwrap()
        .unwrap();
    let state_physical = string(&events[1], "chat_source_ref").unwrap();
    assert!(!seed.members.contains_key(state_physical));
    assert!(saved.members.contains_key(state_physical));
    assert_eq!(saved.state_rows.len(), 1);
    let saved: PreviewOwners =
        serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
    let saved = saved
        .remember(&inactive, "synthetic-admission", &before)
        .unwrap()
        .unwrap();
    assert!(saved.members.contains_key(state_physical));
    let mut without_state = before.clone();
    without_state.state_event_refs.clear();
    let pruned = saved
        .remember(&inactive, "synthetic-admission", &without_state)
        .unwrap()
        .unwrap();
    assert!(pruned.state_rows.is_empty());
    assert!(!pruned.members.contains_key(state_physical));
    let mut contradictory = events[2].clone();
    contradictory.metadata["chat_turn_membership_partial"] = json!(true);
    index
        .observe(
            &mut writer,
            &inactive,
            &contradictory,
            &super::super::super::chat_read::presentation_order(&contradictory).unwrap(),
            None,
        )
        .unwrap();
    assert!(index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &inactive,
            &events[..3],
            Some(&seed),
            &mut SeekCache::default(),
        )
        .unwrap()
        .is_none());
    let retained = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &inactive,
            &events[..3],
            Some(&saved),
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    assert!(retained.proven_openings().is_empty());
    assert!(preview(
        &inactive,
        &events[..3],
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &retained,
    )
    .unwrap()
    .is_some());
    let mut partial = events[..3].to_vec();
    partial[2] = contradictory;
    assert!(preview(
        &inactive,
        &partial,
        "synthetic-admission",
        &BTreeSet::new(),
        true,
        &retained,
    )
    .unwrap()
    .is_none());
}

#[test]
fn certified_members_survive_a_broken_index_without_certifying_new_extensions() {
    for unsupported in [false, true] {
        let admitted = [
            prompt(),
            tool(2, false),
            event(3, AgentChatEventKind::Message),
        ];
        let temp = tempfile::tempdir().unwrap();
        let mut writer = Store::writer(temp.path());
        let mut index = Index::default();
        observe_rows(&mut index, &mut writer, &admitted);
        let owners = index
            .preview_owners_reusing_nodes(
                &mut Store::new(temp.path()),
                &scope(),
                &admitted,
                None,
                &mut SeekCache::default(),
            )
            .unwrap()
            .unwrap();
        let openings = owners.proven_openings().clone();
        assert_eq!(openings, BTreeSet::from(["request-root".into()]));
        let before = preview(&scope(), &admitted, "admission-a", &openings, true, &owners)
            .unwrap()
            .unwrap();
        let saved = owners
            .remember(&scope(), "admission-a", &before)
            .unwrap()
            .unwrap();
        assert!(saved.proven_openings().is_empty());

        let mut broken = event(6, AgentChatEventKind::Status);
        if unsupported {
            broken.kind = AgentChatEventKind::Message;
            broken.role = Some(AgentChatRole::System);
        } else {
            broken.metadata["chat_turn_membership_partial"] = json!(true);
        }
        observe_rows(&mut index, &mut writer, &[broken]);
        let extension = event(7, AgentChatEventKind::Message);
        let result = tool(9, true);
        observe_rows(
            &mut index,
            &mut writer,
            &[extension.clone(), result.clone()],
        );
        let snapshot = serde_json::to_vec(&index).unwrap();

        assert!(index
            .preview_owners_reusing_nodes(
                &mut Store::new(temp.path()),
                &scope(),
                &admitted,
                None,
                &mut SeekCache::default()
            )
            .unwrap()
            .is_none());
        let retained = index
            .preview_owners_reusing_nodes(
                &mut Store::new(temp.path()),
                &scope(),
                &admitted,
                Some(&saved),
                &mut SeekCache::default(),
            )
            .unwrap()
            .unwrap();
        assert!(retained.proven_openings().is_empty());
        assert_eq!(
            preview(
                &scope(),
                &admitted,
                "admission-a",
                &openings,
                true,
                &retained
            )
            .unwrap()
            .unwrap(),
            before
        );
        for new in [
            extension.clone(),
            event(8, AgentChatEventKind::Message),
            result,
        ] {
            assert!(index
                .preview_owners_reusing_nodes(
                    &mut Store::new(temp.path()),
                    &scope(),
                    &[new],
                    Some(&saved),
                    &mut SeekCache::default(),
                )
                .unwrap()
                .is_none());
        }
        let mut merged = admitted[2].clone();
        merged.id = "new-component-on-certified-physical".into();
        merged.metadata["chat_display_member_ids"] = json!([
            admitted[2].metadata["chat_source_ref"],
            extension.metadata["chat_source_ref"],
        ]);
        assert!(index
            .preview_owners_reusing_nodes(
                &mut Store::new(temp.path()),
                &scope(),
                &[merged],
                Some(&saved),
                &mut SeekCache::default(),
            )
            .unwrap()
            .is_none());
        assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
    }
}

#[test]
fn global_turn_order_is_loaded_even_for_an_uncaptured_suffix_and_survives_in_cache() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = Store::writer(temp.path());
    let mut index = Index::default();
    let request = prompt();
    observe_rows(&mut index, &mut writer, std::slice::from_ref(&request));
    let canonical = index
        .page(
            &mut Store::new(temp.path()),
            std::slice::from_ref(&request),
            true,
        )
        .unwrap()
        .unwrap();
    let later = event(9, AgentChatEventKind::Message);
    let owners = index
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            std::slice::from_ref(&later),
            None,
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    assert!(
        owners.rows.is_empty(),
        "the physical suffix is not captured"
    );
    let known = BTreeSet::from(["request-root".into()]);
    let value = preview(
        &scope(),
        std::slice::from_ref(&later),
        "admission-a",
        &known,
        true,
        &owners,
    )
    .unwrap()
    .unwrap();
    assert_eq!(value.turns[0].order_key, canonical.turns[0].order_key);
    let saved = owners
        .remember(&scope(), "admission-a", &value)
        .unwrap()
        .unwrap();
    let next = event(10, AgentChatEventKind::Message);
    let owners = Index::default()
        .preview_owners_reusing_nodes(
            &mut Store::new(temp.path()),
            &scope(),
            std::slice::from_ref(&next),
            Some(&saved),
            &mut SeekCache::default(),
        )
        .unwrap()
        .unwrap();
    let advanced = preview(&scope(), &[next], "admission-a", &known, true, &owners)
        .unwrap()
        .unwrap();
    assert_eq!(advanced.turns[0].order_key, canonical.turns[0].order_key);
    assert!(advanced.turns[0].prompt_item_keys.is_empty());
}

#[test]
fn own_started_state_cannot_replace_the_prompt_as_global_turn_order() {
    let mut started = event(0, AgentChatEventKind::ToolCall);
    started.metadata["raw_type"] = json!("task_started");
    let request = prompt();
    let expected = present(std::slice::from_ref(&request), &BTreeSet::new(), true).unwrap();
    let value = present(&[started.clone(), request], &BTreeSet::new(), true).unwrap();
    assert_eq!(value.turns[0].order_key, expected.turns[0].order_key);
    assert_eq!(value.state_event_refs[0].event_id, started.id);
    assert!(present(&[started], &BTreeSet::from(["request-root".into()]), true).is_none());
}
