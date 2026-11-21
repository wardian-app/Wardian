use super::super::chat_read_store::{Dependency, READ_BYTES, READ_OBJECTS};
use super::*;
use serde_json::json;
use std::path::Path;

fn scope() -> Scope<'static> {
    Scope {
        agent_id: "synthetic-agent",
        conversation_id: "synthetic-conversation",
        source_key: Some("synthetic-source"),
        source_epoch: Some("synthetic-epoch"),
        can_activate: true,
    }
}

fn event(id: &str, kind: AgentChatEventKind, root: Option<&str>) -> AgentChatEvent {
    AgentChatEvent {
        id: id.into(),
        session_id: scope().agent_id.into(),
        provider: "codex".into(),
        kind,
        role: Some(AgentChatRole::Assistant),
        text: Some("Synthetic text; never used as identity or labels".into()),
        title: None,
        status: None,
        turn_id: None,
        source: Some("response_item".into()),
        command: None,
        exit_code: None,
        path: None,
        language: None,
        created_at: Some("2000-01-01T00:00:00Z".into()),
        sequence: None,
        metadata: json!({
            "request_root_id": root,
            "chat_turn_adapter": "codex_bracket_v1",
            "chat_turn_membership_complete": true,
        }),
    }
}

fn prompt(id: &str, root: &str) -> AgentChatEvent {
    let mut event = event(id, AgentChatEventKind::Message, Some(root));
    event.role = Some(AgentChatRole::User);
    event.metadata["input_origin"] = json!("human_input");
    event.metadata["input_purpose"] = json!("request");
    event
}

fn tool(id: &str, root: &str, call: Option<&str>, result: bool) -> AgentChatEvent {
    let kind = if result {
        AgentChatEventKind::ToolResult
    } else {
        AgentChatEventKind::ToolCall
    };
    let mut event = event(id, kind, Some(root));
    event.turn_id = call.map(str::to_owned);
    event.status = Some(if result {
        AgentChatStatus::Succeeded
    } else {
        AgentChatStatus::Running
    });
    if !result {
        event.metadata["tool_name"] = json!("exec_command");
        event.metadata["tool_input"] =
            json!({"description": "Run frontend tests", "cmd": "npm test"});
    }
    event
}

fn approval(id: &str, root: &str, call: Option<&str>) -> AgentChatEvent {
    let mut event = tool(id, root, call, false);
    event.kind = AgentChatEventKind::Approval;
    event.status = Some(AgentChatStatus::ActionRequired);
    event
}

fn observe(index: &mut Index, dir: &Path, event: &AgentChatEvent, order: &str) -> Option<String> {
    index
        .observe(&mut Store::writer(dir), &scope(), event, order, None)
        .unwrap()
}

fn page(
    index: &Index,
    dir: &Path,
    events: &[AgentChatEvent],
    complete: bool,
) -> ChatPresentationV1 {
    let value = index
        .page(&mut Store::new(dir), events, complete)
        .unwrap()
        .expect("qualified page");
    value.validate().unwrap();
    value
}

#[test]
fn empty_index_preserves_legacy_head_serialization() {
    #[derive(Serialize, Deserialize)]
    struct OldHead {
        generation: String,
        #[serde(default, skip_serializing_if = "Index::is_empty")]
        turns: Index,
    }
    let json = br#"{"generation":"unchanged"}"#;
    let head: OldHead = serde_json::from_slice(json).unwrap();
    assert!(head.turns.is_empty());
    assert_eq!(serde_json::to_vec(&head).unwrap(), json);
    assert!(head.turns.node_roots().next().is_none());
    let empty: Index = serde_json::from_str("{}").unwrap();
    assert!(empty.is_empty());
}

#[test]
fn checkpoint_restart_and_replay_preserve_keys_order_and_totals() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("prompt", "request");
    let call = tool("call", "request", Some("call-identity"), false);
    let result = tool("result", "request", Some("call-identity"), true);
    let events = [request, call, result];
    let mut store = Store::checkpoint_writer(temp.path());
    let mut keys = Vec::new();
    for (event, order) in events.iter().zip(["001", "002", "003"]) {
        keys.push(
            index
                .observe(&mut store, &scope(), event, order, None)
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(keys[1], keys[2]);
    assert_eq!(index.node_roots().count(), 3);
    let roots = index
        .node_roots()
        .map(|root| Dependency::Node(root.into()))
        .collect::<Vec<_>>();
    store.finish_checkpoint(&roots).unwrap();
    drop(store);
    let serialized = serde_json::to_vec(&index).unwrap();
    let mut restored: Index = serde_json::from_slice(&serialized).unwrap();
    let before = page(&restored, temp.path(), &events, true);
    assert_eq!(before.items.len(), 2);
    assert_eq!(before.turns[0].step_count, 1);
    assert!(before.turns[0].references_complete);
    for event in &events {
        observe(&mut restored, temp.path(), event, "999");
    }
    assert_eq!(serde_json::to_vec(&restored).unwrap(), serialized);
    assert_eq!(page(&restored, temp.path(), &events, true), before);
    for event in &events[1..] {
        assert_eq!(
            restored
                .step_key(&mut Store::new(temp.path()), &event.id)
                .unwrap(),
            Some(keys[1].clone())
        );
    }
    assert!(restored
        .step_key(&mut Store::new(temp.path()), &events[0].id)
        .unwrap()
        .is_none());
    for event in &events[1..] {
        assert_eq!(
            restored
                .step_output_event(&mut Store::new(temp.path()), &event.id)
                .unwrap()
                .as_deref(),
            Some("result")
        );
    }
}

#[test]
fn parallel_calls_real_retries_and_missing_ids_remain_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let mut failed = tool("result-b", "root", Some("b"), true);
    failed.status = Some(AgentChatStatus::Failed);
    let events = vec![
        prompt("prompt", "root"),
        tool("call-a", "root", Some("a"), false),
        tool("call-b", "root", Some("b"), false),
        failed,
        tool("result-a", "root", Some("a"), true),
        tool("retry-b", "root", Some("b-retry"), false),
        tool("retry-result", "root", Some("b-retry"), true),
        tool("missing-call-a", "root", None, false),
        tool("missing-call-b", "root", None, false),
        tool("missing-result", "root", None, true),
    ];
    let keys = events
        .iter()
        .enumerate()
        .map(|(i, event)| observe(&mut index, temp.path(), event, &format!("{i:04}")).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(keys[1], keys[4]);
    assert_eq!(keys[2], keys[3]);
    assert_eq!(keys[5], keys[6]);
    assert_ne!(keys[2], keys[5]);
    assert_ne!(keys[7], keys[8]);
    assert_ne!(keys[8], keys[9]);
    let page = page(&index, temp.path(), &events, true);
    assert_eq!(page.turns[0].step_count, 6);
    assert_eq!(page.turns[0].failed_step_count, 1);
    let failed = page
        .items
        .iter()
        .find(|item| item.item_key == keys[2])
        .unwrap();
    assert_eq!(failed.step.as_ref().unwrap().status, ChatStepStatus::Failed);
    assert!(page.items.iter().all(|item| item.event_refs.len() <= 2));
}

#[test]
fn result_before_call_uses_first_admitted_order_and_retains_terminal_state() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("prompt", "root");
    observe(&mut index, temp.path(), &request, "001");
    let result = tool("result", "root", Some("call"), true);
    let call = tool("invocation", "root", Some("call"), false);
    let key = index
        .observe(
            &mut Store::writer(temp.path()),
            &scope(),
            &result,
            "009",
            Some("002"),
        )
        .unwrap()
        .unwrap();
    let initial = page(&index, temp.path(), std::slice::from_ref(&result), false);
    assert!(index
        .step_input_event(&mut Store::new(temp.path()), &result.id)
        .unwrap()
        .is_none());
    let order = initial.items[0].order_key.clone();
    assert!(order.starts_with("002:"));
    assert_eq!(
        initial.items[0].step.as_ref().unwrap().description,
        "Run tool"
    );
    assert_eq!(
        index
            .observe(
                &mut Store::writer(temp.path()),
                &scope(),
                &call,
                "003",
                Some("000")
            )
            .unwrap(),
        Some(key.clone())
    );
    let mut stale = result.clone();
    stale.status = Some(AgentChatStatus::Running);
    observe(&mut index, temp.path(), &stale, "010");
    let updated = page(&index, temp.path(), &[request, call, result], true);
    let item = updated
        .items
        .iter()
        .find(|item| item.item_key == key)
        .unwrap();
    assert_eq!(item.order_key, order);
    let step = item.step.as_ref().unwrap();
    assert_eq!(step.description, "Run frontend tests");
    assert_eq!(step.status, ChatStepStatus::Completed);
    assert_eq!(step.invocation_ref.as_deref(), Some("invocation"));
    assert_eq!(step.result_ref.as_deref(), Some("result"));
    assert_eq!(
        index
            .step_input_event(&mut Store::new(temp.path()), "result")
            .unwrap()
            .as_deref(),
        Some("invocation")
    );
    assert_eq!(updated.turns[0].step_count, 1);
}

#[test]
fn only_actual_request_prompts_activate_accepted_roots() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let work = tool("work", "root", Some("call"), false);
    assert!(observe(&mut index, temp.path(), &work, "002").is_some());
    assert!(index
        .page(
            &mut Store::new(temp.path()),
            std::slice::from_ref(&work),
            true
        )
        .unwrap()
        .is_none());
    assert!(index
        .step_key(&mut Store::new(temp.path()), "work")
        .unwrap()
        .is_none());
    for (i, origin, purpose) in [
        (0, "context_injection", "context"),
        (1, "provider_internal", "internal"),
        (2, "human_input", "context"),
        (3, "agent_input", "skill"),
    ] {
        let mut context = prompt(&format!("context-{i}"), &format!("context-root-{i}"));
        context.metadata["input_origin"] = json!(origin);
        context.metadata["input_purpose"] = json!(purpose);
        assert!(observe(&mut index, temp.path(), &context, &format!("00{i}")).is_some());
        assert!(index
            .page(
                &mut Store::new(temp.path()),
                std::slice::from_ref(&work),
                true
            )
            .unwrap()
            .is_none());
    }
    let mut request = prompt("request", "root");
    request.metadata["input_origin"] = json!("agent_input");
    observe(&mut index, temp.path(), &request, "001");
    let value = page(&index, temp.path(), std::slice::from_ref(&work), true);
    assert!(value.turns[0].prompt_item_keys.is_empty());
    assert!(!value.turns[0].references_complete);
    assert!(index
        .step_key(&mut Store::new(temp.path()), "work")
        .unwrap()
        .is_some());
    let mut unrooted = tool("unrooted", "root", Some("root"), false);
    unrooted.metadata = json!({});
    assert!(observe(&mut index, temp.path(), &unrooted, "900").is_none());
    let mixed = page(&index, temp.path(), &[request, unrooted], true);
    assert_eq!(mixed.turns.len(), 1);
    assert_eq!(mixed.legacy_turns.len(), 1);
    assert_eq!(
        mixed.legacy_turns[0].kind,
        ChatLegacyTurnKind::EarlierActivity
    );
}

#[test]
fn roots_are_source_scoped_and_membership_cannot_be_rebound() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request-a", "shared-provider-root");
    let call = tool("call-a", "shared-provider-root", Some("shared-call"), false);
    observe(&mut index, temp.path(), &request, "001");
    let first_key = observe(&mut index, temp.path(), &call, "002").unwrap();
    let source_b = Scope {
        source_key: Some("source-b"),
        ..scope()
    };
    let request_b = prompt("request-b", "shared-provider-root");
    let call_b = tool("call-b", "shared-provider-root", Some("shared-call"), false);
    index
        .observe(
            &mut Store::writer(temp.path()),
            &source_b,
            &request_b,
            "003",
            None,
        )
        .unwrap();
    let second_key = index
        .observe(
            &mut Store::writer(temp.path()),
            &source_b,
            &call_b,
            "004",
            None,
        )
        .unwrap()
        .unwrap();
    assert_ne!(first_key, second_key);
    let value = page(
        &index,
        temp.path(),
        &[request, call.clone(), request_b, call_b],
        true,
    );
    assert_eq!(value.turns.len(), 2);
    let before = serde_json::to_vec(&index).unwrap();
    let mut conflicting = call;
    conflicting.metadata["request_root_id"] = json!("different-root");
    assert!(index
        .observe(
            &mut Store::writer(temp.path()),
            &scope(),
            &conflicting,
            "005",
            None
        )
        .is_err());
    let other_conversation = Scope {
        conversation_id: "different-conversation",
        ..scope()
    };
    assert!(index
        .observe(
            &mut Store::writer(temp.path()),
            &other_conversation,
            &conflicting,
            "006",
            None
        )
        .is_err());
    assert_eq!(serde_json::to_vec(&index).unwrap(), before);
}

#[test]
fn phase_ambiguity_and_idle_never_promote_narration_or_complete_a_turn() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("prompt", "root");
    let mut ambiguous = event("assistant", AgentChatEventKind::Message, Some("root"));
    ambiguous.status = Some(AgentChatStatus::Succeeded);
    let mut idle = event("idle", AgentChatEventKind::Status, Some("root"));
    idle.status = Some(AgentChatStatus::Idle);
    let mut events = vec![request, ambiguous, idle];
    for (i, event) in events.iter_mut().enumerate() {
        event.metadata["chat_turn_membership_complete"] = json!(false);
        observe(&mut index, temp.path(), event, &format!("{i:03}"));
    }
    let initial = page(&index, temp.path(), &events, true);
    assert_eq!(initial.turns[0].status, ChatTurnStatus::Unknown);
    assert!(initial.turns[0].answer_item_keys.is_empty());
    assert_eq!(initial.items[1].kind, ChatPresentedItemKind::Narration);
    for (i, phase) in ["commentary", "future_phase", "final_answer", "final_answer"]
        .iter()
        .enumerate()
    {
        let mut answer = event(
            &format!("phase-{i}"),
            AgentChatEventKind::Message,
            Some("root"),
        );
        answer.metadata["provider_phase"] = json!(phase);
        answer.metadata["chat_turn_membership_complete"] = json!(false);
        answer.status = Some(AgentChatStatus::Succeeded);
        observe(&mut index, temp.path(), &answer, &format!("1{i:02}"));
        events.push(answer);
    }
    let mut stale = events.last().unwrap().clone();
    assert!(!page(&index, temp.path(), &events, true).turns[0].membership_complete);
    let mut closure = event("closure", AgentChatEventKind::Status, Some("root"));
    closure.status = Some(AgentChatStatus::Succeeded);
    observe(&mut index, temp.path(), &closure, "200");
    events.push(closure);
    let final_page = page(&index, temp.path(), &events, true);
    assert_eq!(final_page.turns[0].answer_item_keys.len(), 2);
    assert_eq!(final_page.turns[0].status, ChatTurnStatus::Completed);
    assert!(final_page.turns[0].membership_complete);
    assert_eq!(final_page.turns[0].step_count, 0);
    let snapshot = serde_json::to_vec(&index).unwrap();
    stale.status = Some(AgentChatStatus::Processing);
    observe(&mut index, temp.path(), &stale, "999");
    assert_eq!(page(&index, temp.path(), &events, true), final_page);
    assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
}

#[test]
fn full_totals_survive_older_subsets_without_loading_the_turn() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    observe(&mut index, temp.path(), &request, "0000");
    let events = (0..120)
        .map(|i| {
            let mut event = tool(
                &format!("result-{i}"),
                "root",
                Some(&format!("call-{i}")),
                true,
            );
            if i % 10 == 0 {
                event.status = Some(AgentChatStatus::Failed);
            }
            observe(&mut index, temp.path(), &event, &format!("{:04}", i + 1));
            event
        })
        .collect::<Vec<_>>();
    for subset in [&events[..5], &events[115..], &events[30..40]] {
        let mut reader = Store::new(temp.path());
        let page = index.page(&mut reader, subset, true).unwrap().unwrap();
        assert_eq!(page.turns[0].step_count, 120);
        assert_eq!(page.turns[0].failed_step_count, 12);
        assert!(page.turns[0].counts_complete);
        assert!(!page.turns[0].references_complete);
        assert!(page.turns[0].prompt_item_keys.is_empty());
        assert!(reader.objects < READ_OBJECTS);
        assert!(reader.bytes < READ_BYTES);
        assert!(page.items.len() <= subset.len());
    }
    let partial = page(&index, temp.path(), &events[10..12], false);
    // A full adapter bracket proves membership independently of page readiness.
    assert!(partial.turns[0].counts_complete);
    assert!(partial.turns[0].membership_complete);
    assert_eq!(partial.turns[0].step_count, 120);
    let mut reader = Store::new(temp.path());
    assert!(index
        .page(&mut reader, &events[..81], true)
        .unwrap()
        .is_none());
    assert_eq!(reader.objects, 0);
    let mut inline = events[..80].to_vec();
    for event in &mut inline {
        let (turn_key, item_key) = index
            .event_keys(&mut Store::new(temp.path()), &event.id)
            .unwrap()
            .unwrap();
        event.metadata["chat_turn_key"] = json!(turn_key);
        event.metadata["chat_turn_item_key"] = json!(item_key);
    }
    // Model a shared request whose header reads have already spent half its
    // node/byte allowance. The index must not reset that existing expenditure.
    let mut reader = Store::new(temp.path());
    reader.objects = READ_OBJECTS / 2;
    reader.bytes = READ_BYTES / 2;
    let value = index.page(&mut reader, &inline, true).unwrap().unwrap();
    assert_eq!(value.items.len(), 80);
    assert_eq!(value.turns[0].step_count, 120);
    assert!(reader.objects <= READ_OBJECTS);
    assert!(reader.bytes <= READ_BYTES);
}

#[test]
fn qualified_aliases_and_current_tail_references_bind_one_step() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    let mut call = tool("call", "root", Some("identity"), false);
    call.metadata["chat_source_ref"] = json!("source-coordinate");
    let result = tool("result", "root", Some("identity"), true);
    for (i, event) in [&request, &call, &result].into_iter().enumerate() {
        observe(&mut index, temp.path(), event, &format!("{i:03}"));
    }
    let mut alias = result.clone();
    alias.id = "logical-step".into();
    alias.metadata["chat_display_member_ids"] = json!(["call", "result"]);
    alias.metadata["chat_step_detail_ref"] = json!("row:tail");
    alias.metadata["chat_detail_ref"] = json!("other-row:0");
    let projected = page(&index, temp.path(), &[request.clone(), alias.clone()], true);
    let step = projected.items[1].step.as_ref().unwrap();
    assert_eq!(step.detail_ref.as_deref(), Some("row:tail"));
    assert_eq!(step.detail_availability, ChatDetailAvailability::Available);
    assert_eq!(projected.items[1].event_refs, ["logical-step"]);
    assert_eq!(projected.turns[0].step_count, 1);
    assert!(projected.detail.is_none());
    alias.metadata["chat_step_detail_ref"] = json!("head:row:tail");
    assert_eq!(
        page(&index, temp.path(), &[alias.clone()], true).items[0]
            .step
            .as_ref()
            .unwrap()
            .detail_ref
            .as_deref(),
        Some("head:row:tail")
    );
    alias
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("chat_step_detail_ref");
    assert_eq!(
        page(&index, temp.path(), &[alias.clone()], true).items[0]
            .step
            .as_ref()
            .unwrap()
            .detail_ref
            .as_deref(),
        Some("other-row:tail")
    );
    alias
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("chat_detail_ref");
    let pending = page(&index, temp.path(), &[alias.clone()], true);
    assert!(pending.items[0].step.as_ref().unwrap().detail_ref.is_none());
    assert_eq!(
        pending.items[0].step.as_ref().unwrap().detail_availability,
        ChatDetailAvailability::Pending
    );
    let mut source_alias = call;
    source_alias.id = "qualified-canonical-display".into();
    assert_eq!(
        page(&index, temp.path(), &[source_alias], true).items[0].item_key,
        projected.items[1].item_key
    );
    alias.metadata["chat_display_member_ids"] = json!(["call", "unindexed"]);
    assert!(index
        .page(&mut Store::new(temp.path()), &[alias], true)
        .unwrap()
        .is_none());
    let unknown = tool("unknown", "root", Some("identity"), true);
    // A matching call ID/text alone does not grant an unobserved row membership.
    let mixed = page(&index, temp.path(), &[request, unknown], true);
    assert_eq!(mixed.turns.len(), 1);
    assert_eq!(mixed.legacy_turns[0].event_refs, ["unknown"]);
    assert_eq!(
        mixed.legacy_turns[0].kind,
        ChatLegacyTurnKind::EarlierActivity
    );
}

#[test]
fn labels_are_bounded_adapters_and_output_is_never_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    observe(&mut index, temp.path(), &request, "000");
    let mut read = tool("read", "root", Some("read-call"), false);
    read.metadata["tool_name"] = json!("Read");
    read.metadata["tool_input"] = json!({"file_path": "/synthetic/private/package.json"});
    read.text = Some("RAW_OUTPUT_MUST_NOT_BE_STORED".repeat(1000));
    let mut unknown = tool("custom", "root", Some("custom-call"), false);
    unknown.metadata["tool_name"] = json!("custom_tool");
    unknown.metadata["tool_input"] = json!({"description": "{\"secret\":true}"});
    let mut unsafe_name = tool("unsafe", "root", Some("unsafe-call"), false);
    unsafe_name.metadata["tool_name"] = json!("{\"not_a_tool\":true}");
    unsafe_name.title = Some("Provider lifecycle output".into());
    for (i, event) in [&read, &unknown, &unsafe_name].into_iter().enumerate() {
        observe(&mut index, temp.path(), event, &format!("{:03}", i + 1));
    }
    let value = page(
        &index,
        temp.path(),
        &[read.clone(), unknown, unsafe_name],
        true,
    );
    assert_eq!(
        value.items[0].step.as_ref().unwrap().description,
        "Read package.json"
    );
    assert_eq!(
        value.items[1].step.as_ref().unwrap().description,
        "Run custom_tool"
    );
    assert_eq!(
        value.items[2].step.as_ref().unwrap().description,
        "Run tool"
    );
    let json = serde_json::to_string(&value).unwrap();
    assert!(!json.contains("private"));
    assert!(!json.contains("RAW_OUTPUT_MUST_NOT_BE_STORED"));
    let mut cache = SeekCache::default();
    let mut store = Store::new(temp.path());
    let membership: Membership = lookup(
        &mut store,
        &index.events,
        &digest(read.id.as_bytes()),
        &mut cache,
    )
    .unwrap()
    .unwrap();
    let reference = store
        .get(&index.items, &membership.item_key)
        .unwrap()
        .unwrap();
    let stored = store.read_bytes(&reference, 16 * 1024).unwrap();
    assert!(!String::from_utf8(stored)
        .unwrap()
        .contains("RAW_OUTPUT_MUST_NOT_BE_STORED"));
}

#[test]
fn shared_read_budget_is_not_reset_and_failed_observation_does_not_advance_roots() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    observe(&mut index, temp.path(), &request, "000");
    let before = serde_json::to_vec(&index).unwrap();
    let mut reader = Store::new(temp.path());
    reader.objects = READ_OBJECTS;
    assert!(index
        .page(&mut reader, std::slice::from_ref(&request), true)
        .is_err());
    assert_eq!(reader.objects, READ_OBJECTS);
    let call = tool("call", "root", Some("call"), false);
    assert!(index
        .observe(&mut reader, &scope(), &call, "001", None)
        .is_err());
    assert_eq!(serde_json::to_vec(&index).unwrap(), before);
    let mut reader = Store::new(temp.path());
    reader.bytes = READ_BYTES;
    assert!(index.page(&mut reader, &[request], true).is_err());
}

#[test]
fn archive_only_scope_needs_no_delivery_identity_and_rejects_provisional_rows() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let archive = Scope {
        source_key: None,
        source_epoch: None,
        ..scope()
    };
    let request = prompt("request", "root");
    index
        .observe(
            &mut Store::writer(temp.path()),
            &archive,
            &request,
            "000",
            None,
        )
        .unwrap();
    let value = page(&index, temp.path(), std::slice::from_ref(&request), true);
    assert_eq!(value.items[0].provenance, ChatItemProvenance::Canonical);
    assert!(value.items[0].admission_id.is_none());
    assert!(value.turns[0].duration_ms.is_none());
    assert!(value.turns[0].reported_changes.is_none());
    let mut provisional = request;
    provisional.metadata["chat_provisional"] = json!(true);
    let before = serde_json::to_vec(&index).unwrap();
    assert!(index
        .observe(
            &mut Store::writer(temp.path()),
            &archive,
            &provisional,
            "001",
            None
        )
        .unwrap()
        .is_none());
    assert!(index
        .page(&mut Store::new(temp.path()), &[provisional], true)
        .unwrap()
        .is_none());
    assert_eq!(serde_json::to_vec(&index).unwrap(), before);
}

#[test]
fn provider_turn_identity_is_not_a_call_identity() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "request-root");
    observe(&mut index, temp.path(), &request, "000");
    let mut call_a = tool("call-a", "request-root", Some("provider-turn"), false);
    let mut call_b = tool("call-b", "request-root", Some("provider-turn"), false);
    for event in [&mut call_a, &mut call_b] {
        event.metadata["provider_turn_id"] = json!("provider-turn");
    }
    let key_a = observe(&mut index, temp.path(), &call_a, "001").unwrap();
    let key_b = observe(&mut index, temp.path(), &call_b, "002").unwrap();
    assert_ne!(key_a, key_b);
    let mut explicit = tool("explicit", "request-root", Some("provider-turn"), false);
    explicit.metadata["provider_turn_id"] = json!("provider-turn");
    explicit.metadata["call_id"] = json!("exact-call");
    let mut result = explicit.clone();
    result.id = "explicit-result".into();
    result.kind = AgentChatEventKind::ToolResult;
    result.status = Some(AgentChatStatus::Succeeded);
    assert_eq!(
        observe(&mut index, temp.path(), &explicit, "003"),
        observe(&mut index, temp.path(), &result, "004")
    );
    assert_eq!(
        page(
            &index,
            temp.path(),
            &[call_a, call_b, explicit, result],
            true
        )
        .turns[0]
            .step_count,
        3
    );
}

#[test]
fn saved_invocation_resolves_new_result_without_changing_step_owner() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    let call = tool("canonical-call", "root", Some("identity"), false);
    observe(&mut index, temp.path(), &call, "001");
    assert!(index
        .step_output_event(&mut Store::new(temp.path()), &call.id)
        .unwrap()
        .is_none());
    assert!(index
        .step_input_event(&mut Store::new(temp.path()), &call.id)
        .unwrap()
        .is_none());
    observe(&mut index, temp.path(), &request, "000");
    let owner = index
        .step_key(&mut Store::new(temp.path()), &call.id)
        .unwrap();
    assert_eq!(
        index
            .step_output_event(&mut Store::new(temp.path()), &call.id)
            .unwrap()
            .as_deref(),
        Some("canonical-call")
    );
    let result = tool("canonical-result", "root", Some("identity"), true);
    observe(&mut index, temp.path(), &result, "002");
    let restored: Index = serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
    for event_id in [&call.id, &result.id] {
        assert_eq!(
            restored
                .step_input_event(&mut Store::new(temp.path()), event_id)
                .unwrap()
                .as_deref(),
            Some("canonical-call")
        );
    }
    for event_id in ["unobserved", &request.id] {
        assert!(restored
            .step_input_event(&mut Store::new(temp.path()), event_id)
            .unwrap()
            .is_none());
    }
    assert_eq!(
        restored
            .step_key(&mut Store::new(temp.path()), &call.id)
            .unwrap(),
        owner
    );
    assert_eq!(
        restored
            .step_output_event(&mut Store::new(temp.path()), &call.id)
            .unwrap()
            .as_deref(),
        Some("canonical-result")
    );
    assert!(restored
        .step_output_event(&mut Store::new(temp.path()), "unobserved")
        .unwrap()
        .is_none());
    assert!(restored
        .step_output_event(&mut Store::new(temp.path()), &request.id)
        .unwrap()
        .is_none());
}

#[test]
fn approval_then_invocation_then_result_share_one_ordered_step() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    observe(&mut index, temp.path(), &request, "000");
    let permission = approval("permission", "root", Some("owned-call"));
    let owner = observe(&mut index, temp.path(), &permission, "001").unwrap();
    let initial = page(&index, temp.path(), std::slice::from_ref(&permission), true);
    let order = initial.items[0].order_key.clone();
    let step = initial.items[0].step.as_ref().unwrap();
    assert_eq!(step.status, ChatStepStatus::ActionRequired);
    assert_eq!(step.approval_ref.as_deref(), Some("permission"));
    assert!(step.invocation_ref.is_none());
    assert!(step.result_ref.is_none());
    assert_eq!(initial.turns[0].step_count, 1);
    assert_eq!(initial.turns[0].status, ChatTurnStatus::ActionRequired);
    assert_eq!(
        index
            .step_output_event(&mut Store::new(temp.path()), "permission")
            .unwrap()
            .as_deref(),
        Some("permission")
    );
    assert!(index
        .step_input_event(&mut Store::new(temp.path()), "permission")
        .unwrap()
        .is_none());
    let invocation = tool("invocation", "root", Some("owned-call"), false);
    assert_eq!(
        observe(&mut index, temp.path(), &invocation, "002"),
        Some(owner.clone())
    );
    assert_eq!(
        index
            .step_status(&mut Store::new(temp.path()), "permission")
            .unwrap(),
        Some(ChatStepStatus::Running)
    );
    observe(&mut index, temp.path(), &permission, "999");
    let working = page(
        &index,
        temp.path(),
        &[permission.clone(), invocation.clone()],
        true,
    );
    assert_eq!(
        working.items[0].step.as_ref().unwrap().status,
        ChatStepStatus::Running
    );
    assert_eq!(working.turns[0].status, ChatTurnStatus::Running);
    let result = tool("result", "root", Some("owned-call"), true);
    assert_eq!(
        observe(&mut index, temp.path(), &result, "003"),
        Some(owner.clone())
    );
    let final_page = page(
        &index,
        temp.path(),
        &[request, permission, invocation, result],
        true,
    );
    let item = final_page
        .items
        .iter()
        .find(|item| item.item_key == owner)
        .unwrap();
    let step = item.step.as_ref().unwrap();
    assert_eq!(item.order_key, order);
    assert_eq!(item.event_refs, ["permission", "invocation", "result"]);
    assert_eq!(step.status, ChatStepStatus::Completed);
    assert_eq!(step.approval_ref.as_deref(), Some("permission"));
    assert_eq!(step.invocation_ref.as_deref(), Some("invocation"));
    assert_eq!(step.result_ref.as_deref(), Some("result"));
    assert_eq!(final_page.turns[0].step_count, 1);
    assert_eq!(final_page.turns[0].failed_step_count, 0);
    assert_eq!(
        index
            .step_status(&mut Store::new(temp.path()), "permission")
            .unwrap(),
        Some(ChatStepStatus::Completed)
    );
    assert_eq!(
        index
            .step_output_event(&mut Store::new(temp.path()), "permission")
            .unwrap()
            .as_deref(),
        Some("result")
    );
}

#[test]
fn concurrent_approvals_require_distinct_owned_call_identities() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    observe(&mut index, temp.path(), &prompt("request", "root"), "000");
    let first = approval("permission-a", "root", Some("call-a"));
    let second = approval("permission-b", "root", Some("call-b"));
    let missing_a = approval("permission-missing-a", "root", None);
    let missing_b = approval("permission-missing-b", "root", None);
    let mut keys = Vec::new();
    for (i, event) in [&first, &second, &missing_a, &missing_b]
        .into_iter()
        .enumerate()
    {
        keys.push(observe(&mut index, temp.path(), event, &format!("{:03}", i + 1)).unwrap());
    }
    assert_eq!(
        keys.iter().collect::<std::collections::BTreeSet<_>>().len(),
        4
    );
    let invocation = tool("invocation-a", "root", Some("call-a"), false);
    assert_eq!(
        observe(&mut index, temp.path(), &invocation, "005"),
        Some(keys[0].clone())
    );
    let value = page(
        &index,
        temp.path(),
        &[first, second, missing_a, missing_b, invocation],
        true,
    );
    assert_eq!(value.turns[0].step_count, 4);
    assert_eq!(value.turns[0].status, ChatTurnStatus::ActionRequired);
    assert_eq!(
        index
            .step_status(&mut Store::new(temp.path()), "permission-a")
            .unwrap(),
        Some(ChatStepStatus::Running)
    );
    assert_eq!(
        index
            .step_status(&mut Store::new(temp.path()), "permission-b")
            .unwrap(),
        Some(ChatStepStatus::ActionRequired)
    );
    let mut unrooted = approval("unrooted", "root", Some("call-a"));
    unrooted
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("request_root_id");
    assert!(observe(&mut index, temp.path(), &unrooted, "006").is_none());
    assert!(index
        .page(&mut Store::new(temp.path()), &[unrooted], true)
        .unwrap()
        .is_none());
}

#[test]
fn proven_turn_termination_stops_only_unresolved_steps() {
    for (terminal, expected, expected_turn) in [
        (
            AgentChatStatus::Cancelled,
            ChatStepStatus::Cancelled,
            ChatTurnStatus::Cancelled,
        ),
        (
            AgentChatStatus::Failed,
            ChatStepStatus::Incomplete,
            ChatTurnStatus::Failed,
        ),
        (
            AgentChatStatus::Succeeded,
            ChatStepStatus::Incomplete,
            ChatTurnStatus::Completed,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut index = Index::default();
        observe(&mut index, temp.path(), &prompt("request", "root"), "000");
        let running = tool("running", "root", Some("running-call"), false);
        let permission = approval("approval", "root", Some("approval-call"));
        let mut unknown = tool("unknown", "root", Some("unknown-call"), false);
        unknown.status = None;
        let mut failed = tool("failed", "root", Some("failed-call"), true);
        failed.status = Some(AgentChatStatus::Failed);
        let completed = tool("completed", "root", Some("completed-call"), true);
        let mut cancelled = tool("cancelled", "root", Some("cancelled-call"), false);
        cancelled.status = Some(AgentChatStatus::Cancelled);
        let events = [running, permission, unknown, failed, completed, cancelled];
        for (i, event) in events.iter().enumerate() {
            observe(&mut index, temp.path(), event, &format!("{:03}", i + 1));
        }
        let mut end = event("termination", AgentChatEventKind::Status, Some("root"));
        end.status = Some(terminal);
        observe(&mut index, temp.path(), &end, "010");
        let restored: Index = serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
        let value = page(&restored, temp.path(), &events, true);
        assert_eq!(value.turns[0].status, expected_turn);
        assert_eq!(value.turns[0].step_count, 6);
        assert_eq!(value.turns[0].failed_step_count, 1);
        for (i, event) in events.iter().enumerate() {
            let expected = match i {
                0..=2 => expected.clone(),
                3 => ChatStepStatus::Failed,
                4 => ChatStepStatus::Completed,
                _ => ChatStepStatus::Cancelled,
            };
            let mut reader = Store::new(temp.path());
            assert_eq!(
                restored.step_status(&mut reader, &event.id).unwrap(),
                Some(expected.clone())
            );
            assert_eq!(value.items[i].step.as_ref().unwrap().status, expected);
            assert!(reader.objects < READ_OBJECTS);
        }
        assert!(restored
            .step_status(&mut Store::new(temp.path()), "termination")
            .unwrap()
            .is_none());
        assert!(restored
            .step_status(&mut Store::new(temp.path()), "unobserved")
            .unwrap()
            .is_none());
        let mut cache = SeekCache::default();
        let mut store = Store::new(temp.path());
        let member: Membership = lookup(
            &mut store,
            &restored.events,
            &digest(b"running"),
            &mut cache,
        )
        .unwrap()
        .unwrap();
        let item: Item = lookup(&mut store, &restored.items, &member.item_key, &mut cache)
            .unwrap()
            .unwrap();
        let mut turn: Turn = lookup(&mut store, &restored.turns, &member.turn_key, &mut cache)
            .unwrap()
            .unwrap();
        turn.status = ChatTurnStatus::Incomplete;
        assert_eq!(
            resolved_step_status(item.step.as_ref().unwrap(), &turn),
            ChatStepStatus::Incomplete
        );
    }
}

#[test]
fn result_replay_in_reverse_keeps_latest_backing_and_failure_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    observe(&mut index, temp.path(), &prompt("request", "root"), "000");
    let call = tool("invocation", "root", Some("call"), false);
    let owner = observe(&mut index, temp.path(), &call, "001").unwrap();
    let first = tool("result-first", "root", Some("call"), true);
    let mut middle = tool("result-failed", "root", Some("call"), true);
    middle.status = Some(AgentChatStatus::Failed);
    let last = tool("result-latest", "root", Some("call"), true);
    for (i, result) in [&first, &middle, &last].into_iter().enumerate() {
        assert_eq!(
            observe(&mut index, temp.path(), result, &format!("{:03}", i + 2)),
            Some(owner.clone())
        );
    }
    let events = [call, first, middle, last];
    let before = page(&index, temp.path(), &events, true);
    let snapshot = serde_json::to_vec(&index).unwrap();
    let mut restored: Index = serde_json::from_slice(&snapshot).unwrap();
    for (i, event) in events.iter().rev().enumerate() {
        assert_eq!(
            observe(&mut restored, temp.path(), event, &format!("9{i:02}")),
            Some(owner.clone())
        );
        assert_eq!(
            restored
                .step_output_event(&mut Store::new(temp.path()), &event.id)
                .unwrap()
                .as_deref(),
            Some("result-latest")
        );
    }
    assert_eq!(serde_json::to_vec(&restored).unwrap(), snapshot);
    let after = page(&restored, temp.path(), &events, true);
    assert_eq!(after, before);
    assert_eq!(after.items.len(), 1);
    assert_eq!(after.items[0].event_refs.len(), 4);
    assert_eq!(after.items[0].item_key, owner);
    assert_eq!(
        after.items[0].step.as_ref().unwrap().result_ref.as_deref(),
        Some("result-latest")
    );
    assert_eq!(
        after.items[0].step.as_ref().unwrap().status,
        ChatStepStatus::Failed
    );
    assert_eq!(after.turns[0].step_count, 1);
    assert_eq!(after.turns[0].failed_step_count, 1);
}

#[test]
fn approval_failure_remains_failed_after_later_successful_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    observe(&mut index, temp.path(), &prompt("request", "root"), "000");
    let mut denied = approval("denied", "root", Some("call"));
    denied.status = Some(AgentChatStatus::Failed);
    observe(&mut index, temp.path(), &denied, "001");
    let invocation = tool("invocation", "root", Some("call"), false);
    let result = tool("result", "root", Some("call"), true);
    observe(&mut index, temp.path(), &invocation, "002");
    observe(&mut index, temp.path(), &result, "003");
    let value = page(&index, temp.path(), &[denied, invocation, result], true);
    assert_eq!(
        value.items[0].step.as_ref().unwrap().status,
        ChatStepStatus::Failed
    );
    assert_eq!(value.turns[0].failed_step_count, 1);
    assert_eq!(
        index
            .step_status(&mut Store::new(temp.path()), "denied")
            .unwrap(),
        Some(ChatStepStatus::Failed)
    );
}

#[test]
fn changed_legacy_rows_and_late_prompt_mirrors_cannot_activate_partial_totals() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let legacy = Scope {
        can_activate: false,
        ..scope()
    };
    let old_prompt = prompt("old-prompt", "old-root");
    let changed = tool("changed-step", "old-root", Some("old-call"), true);
    let added = tool("new-step", "old-root", Some("new-call"), false);
    let mirror = prompt("late-mirror", "old-root");
    let old_events = [old_prompt, changed, added, mirror];
    for (i, event) in old_events.iter().enumerate() {
        index
            .observe(
                &mut Store::writer(temp.path()),
                &legacy,
                event,
                &format!("{i:03}"),
                None,
            )
            .unwrap();
        assert!(index
            .page(&mut Store::new(temp.path()), &old_events[..=i], true)
            .unwrap()
            .is_none());
    }
    assert!(index
        .step_status(&mut Store::new(temp.path()), "new-step")
        .unwrap()
        .is_none());
    let restored: Index = serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
    assert!(restored
        .page(&mut Store::new(temp.path()), &old_events, true)
        .unwrap()
        .is_none());
    let new_prompt = prompt("admitted-new-prompt", "new-root");
    observe(&mut index, temp.path(), &new_prompt, "010");
    let new_step = tool("admitted-new-step", "new-root", Some("new-call"), false);
    index
        .observe(
            &mut Store::writer(temp.path()),
            &legacy,
            &new_step,
            "011",
            None,
        )
        .unwrap();
    let value = page(&index, temp.path(), &[new_prompt.clone(), new_step], true);
    assert_eq!(value.turns[0].step_count, 1);
    assert!(value.turns[0].counts_complete);
    let mixed = page(
        &index,
        temp.path(),
        &[old_events[0].clone(), old_events[1].clone(), new_prompt],
        true,
    );
    assert_eq!(mixed.turns.len(), 1);
    assert_eq!(mixed.legacy_turns.len(), 1);
    assert_eq!(mixed.legacy_turns[0].kind, ChatLegacyTurnKind::Unmigrated);
    assert_eq!(
        mixed.legacy_turns[0].event_refs,
        ["old-prompt", "changed-step"]
    );
    assert!(!mixed.legacy_turns[0].membership_complete);
    assert_eq!(
        mixed.legacy_turns[0].turn_key,
        index
            .event_keys(&mut Store::new(temp.path()), "old-prompt")
            .unwrap()
            .unwrap()
            .0
    );
}

#[test]
fn unreliable_phase_replay_preserves_answer_and_prompt_kind_changes_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let mut request = prompt("request", "root");
    request.metadata["chat_turn_membership_complete"] = json!(false);
    observe(&mut index, temp.path(), &request, "000");
    let mut narration = event("narration", AgentChatEventKind::Message, Some("root"));
    narration.metadata["chat_turn_membership_complete"] = json!(false);
    observe(&mut index, temp.path(), &narration, "001");
    let mut answer = event("answer", AgentChatEventKind::Message, Some("root"));
    answer.metadata["provider_phase"] = json!("final_answer");
    answer.metadata["chat_turn_membership_complete"] = json!(false);
    answer.status = Some(AgentChatStatus::Succeeded);
    observe(&mut index, temp.path(), &answer, "002");
    let mut closure = event("closure", AgentChatEventKind::Status, Some("root"));
    closure.status = Some(AgentChatStatus::Succeeded);
    observe(&mut index, temp.path(), &closure, "003");
    let before = page(
        &index,
        temp.path(),
        &[request.clone(), answer.clone()],
        true,
    );
    for phase in [None, Some("future_phase")] {
        let mut stale = answer.clone();
        stale.metadata["provider_phase"] = json!(phase);
        stale.status = None;
        observe(&mut index, temp.path(), &stale, "999");
        let value = page(&index, temp.path(), &[request.clone(), stale], true);
        assert_eq!(value, before);
    }
    let mut demoted = request.clone();
    demoted.role = Some(AgentChatRole::Assistant);
    let snapshot = serde_json::to_vec(&index).unwrap();
    assert!(index
        .observe(
            &mut Store::writer(temp.path()),
            &scope(),
            &demoted,
            "004",
            None
        )
        .is_err());
    assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
    let promoted = prompt("narration", "root");
    let snapshot = serde_json::to_vec(&index).unwrap();
    assert!(index
        .observe(
            &mut Store::writer(temp.path()),
            &scope(),
            &promoted,
            "005",
            None
        )
        .is_err());
    assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
    let mut commentary = answer;
    commentary.metadata["provider_phase"] = json!("commentary");
    observe(&mut index, temp.path(), &commentary, "006");
    let value = page(&index, temp.path(), &[request, commentary], true);
    assert_eq!(value, before);
    assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
}

#[test]
fn unsupported_shapes_keep_the_whole_owned_turn_legacy_without_disabling_other_turns() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    observe(&mut index, temp.path(), &request, "000");
    let work = tool("old-work", "root", Some("owned-call"), false);
    observe(&mut index, temp.path(), &work, "001");
    let state = event("old-status", AgentChatEventKind::Status, Some("root"));
    observe(&mut index, temp.path(), &state, "002");
    let original_order = page(&index, temp.path(), std::slice::from_ref(&request), true).turns[0]
        .order_key
        .clone();
    let eligible = prompt("eligible-request", "eligible-root");
    observe(&mut index, temp.path(), &eligible, "100");
    let mut system = event("system", AgentChatEventKind::Message, Some("root"));
    system.role = Some(AgentChatRole::System);
    let mut context = event("generic-context", AgentChatEventKind::Message, Some("root"));
    context.role = Some(AgentChatRole::System);
    context.metadata["input_purpose"] = json!("context");
    let mut tool_message = event("tool-message", AgentChatEventKind::Message, Some("root"));
    tool_message.role = Some(AgentChatRole::Tool);
    for mut unsupported in [
        system,
        context,
        tool_message,
        event("error", AgentChatEventKind::Error, Some("root")),
        event("terminal", AgentChatEventKind::TerminalOutput, Some("root")),
        event("memory", AgentChatEventKind::Memory, Some("root")),
    ] {
        unsupported.text = Some(r#"{"synthetic_diagnostic":"retain source visibility"}"#.into());
        let unchanged = unsupported.clone();
        assert!(observe(&mut index, temp.path(), &unsupported, "010").is_some());
        let mixed = page(
            &index,
            temp.path(),
            &[
                request.clone(),
                work.clone(),
                state.clone(),
                unsupported.clone(),
                eligible.clone(),
            ],
            true,
        );
        assert_eq!(mixed.turns.len(), 1);
        assert_eq!(mixed.items.len(), 1);
        assert_eq!(mixed.items[0].event_refs, ["eligible-request"]);
        assert!(mixed.state_event_refs.is_empty());
        assert_eq!(mixed.legacy_turns.len(), 1);
        assert_eq!(mixed.legacy_turns[0].kind, ChatLegacyTurnKind::Unmigrated);
        assert_eq!(mixed.legacy_turns[0].order_key, original_order);
        assert_eq!(
            mixed.legacy_turns[0].event_refs,
            vec![
                request.id.clone(),
                work.id.clone(),
                state.id.clone(),
                unsupported.id.clone()
            ]
        );
        assert!(!mixed.legacy_turns[0].membership_complete);
        assert_eq!(
            mixed.legacy_turns[0].turn_key,
            index
                .event_keys(&mut Store::new(temp.path()), &unsupported.id)
                .unwrap()
                .unwrap()
                .0
        );
        assert!(index
            .step_key(&mut Store::new(temp.path()), &unsupported.id)
            .unwrap()
            .is_none());
        assert_eq!(unsupported, unchanged);
        assert!(index
            .step_key(&mut Store::new(temp.path()), &work.id)
            .unwrap()
            .is_none());
    }
    // Even a formerly indexed ID cannot grant visibility to a now unsupported
    // current shape through its old metadata membership.
    let mut changed = request.clone();
    changed.role = Some(AgentChatRole::System);
    assert!(observe(&mut index, temp.path(), &changed, "020").is_some());
    assert!(index
        .page(&mut Store::new(temp.path()), &[changed], true)
        .unwrap()
        .is_none());
    observe(&mut index, temp.path(), &request, "021");
    let restored: Index = serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
    let narrow = page(&restored, temp.path(), &[request.clone(), eligible], true);
    assert_eq!(narrow.legacy_turns[0].event_refs, ["request"]);
    assert_eq!(narrow.legacy_turns[0].order_key, original_order);
    assert_eq!(narrow.items.len(), 1);
    assert!(restored
        .page(&mut Store::new(temp.path()), &[request], true)
        .unwrap()
        .is_none());
}

#[test]
fn classified_internal_context_is_state_evidence_and_never_assistant_prose() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    observe(&mut index, temp.path(), &request, "000");
    let mut events = vec![request];
    for origin in ["provider_internal", "context_injection"] {
        for (i, role) in [
            AgentChatRole::User,
            AgentChatRole::System,
            AgentChatRole::Assistant,
        ]
        .into_iter()
        .enumerate()
        {
            let mut context = event(
                &format!("{origin}-{i}"),
                AgentChatEventKind::Message,
                Some("root"),
            );
            context.role = Some(role);
            context.metadata["input_origin"] = json!(origin);
            context.metadata["input_purpose"] = json!("context");
            context.metadata["provider_phase"] = json!("final_answer");
            context.status = Some(AgentChatStatus::Succeeded);
            assert!(observe(&mut index, temp.path(), &context, &format!("1{origin}{i}")).is_some());
            events.push(context);
        }
    }
    let value = page(&index, temp.path(), &events, true);
    assert_eq!(value.items.len(), 1);
    assert_eq!(value.items[0].kind, ChatPresentedItemKind::Prompt);
    assert_eq!(value.state_event_refs.len(), 6);
    assert!(value.legacy_turns.is_empty());
    assert_eq!(value.turns[0].step_count, 0);
    assert_eq!(value.turns[0].status, ChatTurnStatus::Unknown);
    assert!(!value.turns[0].answer_recorded);
    assert!(value.turns[0].answer_item_keys.is_empty());
    for reference in &value.state_event_refs {
        assert_eq!(reference.turn_key, value.turns[0].turn_key);
    }
}

#[test]
fn earlier_activity_group_is_scope_stable_across_pages_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("new-request", "new-root");
    observe(&mut index, temp.path(), &request, "100");
    // A clipped header's root-looking field does not establish membership.
    let mut old_a = event("old-a", AgentChatEventKind::Message, Some("new-root"));
    old_a.role = Some(AgentChatRole::System);
    let old_b = event("old-b", AgentChatEventKind::Memory, None);
    let first = page(&index, temp.path(), &[old_a.clone(), request.clone()], true);
    let restored: Index = serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
    let second = page(
        &restored,
        temp.path(),
        &[old_b.clone(), request.clone()],
        true,
    );
    assert_eq!(first.legacy_turns.len(), 1);
    assert_eq!(
        first.legacy_turns[0].kind,
        ChatLegacyTurnKind::EarlierActivity
    );
    assert_eq!(first.legacy_turns[0].order_key, "0:earlier");
    assert!(!first.legacy_turns[0].membership_complete);
    assert_eq!(
        first.legacy_turns[0].turn_key,
        second.legacy_turns[0].turn_key
    );
    assert_eq!(first.turns[0].order_key, second.turns[0].order_key);
    assert_eq!(first.items[0].event_refs, ["new-request"]);
    assert_eq!(first.legacy_turns[0].event_refs, ["old-a"]);
    assert_eq!(second.legacy_turns[0].event_refs, ["old-b"]);
    let combined = page(
        &restored,
        temp.path(),
        &[old_a.clone(), old_b.clone(), request.clone()],
        true,
    );
    assert_eq!(combined.legacy_turns[0].event_refs, ["old-a", "old-b"]);
    assert_eq!(combined.items.len(), 1);
    let mut refs = combined
        .items
        .iter()
        .flat_map(|item| item.event_refs.iter())
        .chain(
            combined
                .legacy_turns
                .iter()
                .flat_map(|turn| turn.event_refs.iter()),
        )
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    assert_eq!(refs.len(), 3);
    assert!(restored
        .page(&mut Store::new(temp.path()), &[old_a, old_b], true)
        .unwrap()
        .is_none());
}

#[test]
fn event_keys_are_version_guarded_and_inline_rows_retain_membership_validation() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let call = tool("call", "root", Some("owned-call"), false);
    let key = observe(&mut index, temp.path(), &call, "001").unwrap();
    let (turn_key, item_key) = index
        .event_keys(&mut Store::new(temp.path()), &call.id)
        .unwrap()
        .unwrap();
    assert_eq!(item_key, key);
    let mut inline = call.clone();
    inline.metadata["chat_turn_key"] = json!(turn_key);
    inline.metadata["chat_turn_item_key"] = json!(item_key);
    assert!(index
        .page(
            &mut Store::new(temp.path()),
            std::slice::from_ref(&inline),
            true
        )
        .unwrap()
        .is_none());
    observe(&mut index, temp.path(), &prompt("request", "root"), "000");
    let expected = page(&index, temp.path(), std::slice::from_ref(&call), true);
    assert_eq!(
        page(&index, temp.path(), std::slice::from_ref(&inline), true),
        expected
    );
    let mut no_event_tree = index.clone();
    no_event_tree.events = None;
    assert_eq!(
        page(
            &no_event_tree,
            temp.path(),
            std::slice::from_ref(&inline),
            true
        ),
        expected
    );
    assert!(no_event_tree
        .page(
            &mut Store::new(temp.path()),
            std::slice::from_ref(&call),
            true
        )
        .unwrap()
        .is_none());
    let mut wrong_turn = inline.clone();
    wrong_turn.metadata["chat_turn_key"] = json!("a".repeat(64));
    assert!(index
        .page(&mut Store::new(temp.path()), &[wrong_turn], true)
        .unwrap()
        .is_none());
    let mut oversized = inline.clone();
    oversized.metadata["chat_turn_item_key"] = json!("a".repeat(ID_BYTES + 1));
    assert!(no_event_tree
        .page(&mut Store::new(temp.path()), &[oversized], true)
        .unwrap()
        .is_none());
    let mut alias = inline.clone();
    alias.metadata["chat_display_member_ids"] = json!(["call"]);
    alias.metadata["chat_turn_key"] = json!("b".repeat(64));
    assert_eq!(page(&index, temp.path(), &[alias.clone()], true), expected);
    alias.metadata["chat_display_member_ids"] = json!(["unobserved"]);
    assert!(index
        .page(&mut Store::new(temp.path()), &[alias], true)
        .unwrap()
        .is_none());
    assert!(index
        .event_keys(&mut Store::new(temp.path()), "unobserved")
        .unwrap()
        .is_none());
    let mut unsupported = index;
    unsupported.version = 2;
    let mut reader = Store::new(temp.path());
    reader.objects = READ_OBJECTS;
    assert!(unsupported
        .event_keys(&mut reader, "call")
        .unwrap()
        .is_none());
    assert_eq!(reader.objects, READ_OBJECTS);
}

#[test]
fn canonical_prompt_provider_enrichment_preserves_scope_keys_order_and_totals() {
    for scope in [
        scope(),
        Scope {
            source_key: None,
            source_epoch: None,
            ..scope()
        },
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut index = Index::default();
        let mut request = prompt("canonical-request", "accepted-root");
        request.provider = "unknown".into();
        request.metadata["generated"] = json!(true);
        let prompt_key = index
            .observe(
                &mut Store::writer(temp.path()),
                &scope,
                &request,
                "001",
                None,
            )
            .unwrap()
            .unwrap();
        let call = tool("canonical-call", "accepted-root", Some("owned-call"), false);
        index
            .observe(&mut Store::writer(temp.path()), &scope, &call, "002", None)
            .unwrap();
        let keys = index
            .event_keys(&mut Store::new(temp.path()), &request.id)
            .unwrap()
            .unwrap();
        assert!(index
            .page(
                &mut Store::new(temp.path()),
                &[request.clone(), call.clone()],
                true
            )
            .unwrap()
            .is_none());
        let mut cache = SeekCache::default();
        let mut store = Store::new(temp.path());
        let before: Item = lookup(&mut store, &index.items, &keys.1, &mut cache)
            .unwrap()
            .unwrap();
        request.provider = "codex".into();
        assert_eq!(
            index
                .observe(
                    &mut Store::writer(temp.path()),
                    &scope,
                    &request,
                    "999",
                    None
                )
                .unwrap(),
            Some(prompt_key)
        );
        assert_eq!(
            index
                .event_keys(&mut Store::new(temp.path()), &request.id)
                .unwrap(),
            Some(keys.clone())
        );
        assert_eq!(
            index
                .event_keys(&mut Store::new(temp.path()), &call.id)
                .unwrap()
                .unwrap()
                .0,
            keys.0
        );
        let after = page(&index, temp.path(), &[request, call], true);
        assert_eq!(after.items[0].item_key, before.membership.item_key);
        assert_eq!(after.items[0].order_key, before.order_key);
        assert_eq!(after.turns.len(), 1);
        assert_eq!(after.turns[0].prompt_item_keys.len(), 1);
        assert_eq!(after.turns[0].step_count, 1);
    }
}

#[test]
fn normalized_turn_start_markers_are_state_while_exec_start_records_are_steps() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    observe(&mut index, temp.path(), &request, "000");
    let mut events = vec![request];
    for (i, raw_type) in [
        "task_started",
        "turn_started",
        "exec_command_begin",
        "exec_command_start",
    ]
    .into_iter()
    .enumerate()
    {
        let mut normalized = tool(raw_type, "root", Some(raw_type), false);
        normalized.metadata["raw_type"] = json!(raw_type);
        normalized.status = Some(AgentChatStatus::Running);
        assert!(observe(
            &mut index,
            temp.path(),
            &normalized,
            &format!("{:03}", i + 1)
        )
        .is_some());
        events.push(normalized);
        if i < 2 {
            let started = page(&index, temp.path(), &events, true);
            assert_eq!(started.turns[0].status, ChatTurnStatus::Running);
            assert_eq!(started.turns[0].step_count, 0);
        }
    }
    let value = page(&index, temp.path(), &events, true);
    assert_eq!(value.turns[0].step_count, 2);
    assert_eq!(
        value
            .state_event_refs
            .iter()
            .map(|reference| reference.event_id.as_str())
            .collect::<Vec<_>>(),
        ["task_started", "turn_started"]
    );
    let steps = value
        .items
        .iter()
        .filter(|item| item.kind == ChatPresentedItemKind::Step)
        .collect::<Vec<_>>();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].event_refs, ["exec_command_begin"]);
    assert_eq!(steps[1].event_refs, ["exec_command_start"]);
    for marker in ["task_started", "turn_started"] {
        assert!(index
            .step_key(&mut Store::new(temp.path()), marker)
            .unwrap()
            .is_none());
        assert!(index
            .event_keys(&mut Store::new(temp.path()), marker)
            .unwrap()
            .is_some());
    }
    assert!(value.legacy_turns.is_empty());
}

#[test]
fn certified_unphased_and_final_mirrors_keep_the_earliest_answer_owner_across_older_and_restart() {
    use super::super::chat_logical_index::{Admission, Index as LogicalIndex};
    use super::super::ConversationArchiveContext;
    use crate::providers::chat_transcript::normalize_chat_lines;

    let temp = tempfile::tempdir().unwrap();
    let raw = [
        r#"{"type":"event_msg","payload":{"type":"agent_message","message":"Synthetic final answer.","internal_chat_message_metadata_passthrough":{"turn_id":"provider-turn"}}}"#,
        r#"{"type":"response_item","payload":{"type":"message","id":"native-final","role":"assistant","phase":"final_answer","text":"Synthetic final answer.","internal_chat_message_metadata_passthrough":{"turn_id":"provider-turn"}}}"#,
    ];
    let log_path = temp.path().join("synthetic.jsonl");
    let source = raw.join("\n");
    std::fs::write(&log_path, &source).unwrap();
    let path = log_path
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let epoch = "e".repeat(64);
    let mut context = ConversationArchiveContext::for_agent_id(scope().agent_id, "codex");
    context.provider_source_key = Some("synthetic-source".into());
    context.provider_session_ids = vec!["native-session".into()];
    let admission = Admission {
        context: &context,
        conversation: Some(scope().conversation_id),
        epoch: &epoch,
        admission: "synthetic-policy",
        path: &path,
        watermark: source.len() as u64,
        sequence_trusted: true,
    };
    let mut mirrors = normalize_chat_lines(scope().agent_id, "codex", raw);
    assert_eq!(mirrors.len(), 2);
    assert!(mirrors[0].metadata["provider_phase"].is_null());
    assert_eq!(mirrors[1].metadata["provider_phase"], "final_answer");
    let mut offset = 0;
    for (event, frame) in mirrors.iter_mut().zip(raw) {
        event.id = format!(
            "source:{}:{epoch}:{offset}:{}:0",
            scope().agent_id,
            digest(frame.as_bytes())
        );
        event.metadata["request_root_id"] = json!("accepted-root");
        event.metadata["provider_log"] = json!(true);
        event.metadata["provider_session_id"] = json!("native-session");
        event.metadata["log_path"] = json!(&path);
        event.metadata["chat_source_ref"] = json!(event.id);
        event.metadata["chat_source_epoch"] = json!(&epoch);
        event.metadata["chat_source_start"] = json!(offset);
        event.metadata["chat_source_end"] = json!(offset + frame.len());
        event.metadata["chat_turn_adapter"] = json!("codex_bracket_v1");
        event.metadata["chat_turn_membership_complete"] = json!(true);
        offset += frame.len() + 1;
    }
    let mut logical = LogicalIndex::default();
    let mut certificate = None;
    for (i, event) in mirrors.iter().enumerate() {
        let updates = logical
            .observe(
                &mut Store::writer(temp.path()),
                &admission,
                event,
                &format!("row-{i}"),
            )
            .unwrap();
        for (_, relation) in updates {
            if relation.is_some() {
                certificate = relation;
            }
        }
    }
    let certificate =
        certificate.expect("existing logical owner certifies native mirror component");
    assert_eq!(
        certificate.members.iter().collect::<BTreeSet<_>>(),
        mirrors
            .iter()
            .map(|event| &event.id)
            .collect::<BTreeSet<_>>()
    );
    let scope = Scope {
        source_epoch: Some(&epoch),
        ..scope()
    };
    let mut index = Index::default();
    let request = prompt("request", "accepted-root");
    index
        .observe(
            &mut Store::writer(temp.path()),
            &scope,
            &request,
            "000",
            None,
        )
        .unwrap();
    let owner = index
        .observe(
            &mut Store::writer(temp.path()),
            &scope,
            &mirrors[0],
            "001",
            None,
        )
        .unwrap()
        .unwrap();
    let initial_version = page(&index, temp.path(), &[mirrors[0].clone()], true).items[0]
        .evidence_version
        .clone();
    index
        .observe(
            &mut Store::writer(temp.path()),
            &scope,
            &mirrors[1],
            "002",
            None,
        )
        .unwrap();
    let snapshot = serde_json::to_vec(&index).unwrap();
    let mut composite = mirrors[1].clone();
    composite.id = certificate.id.clone();
    composite.metadata["chat_display_member_ids"] = json!(&certificate.members);
    let first = page(&index, temp.path(), &[request, composite.clone()], true);
    assert_eq!(first.items.len(), 2);
    assert_eq!(
        first.turns[0].answer_item_keys.as_slice(),
        std::slice::from_ref(&owner)
    );
    assert!(first.turns[0].answer_recorded);
    assert_eq!(first.turns[0].step_count, 0);
    assert_eq!(first.turns[0].failed_step_count, 0);
    assert!(!first.turns[0].references_complete);
    let answer = first
        .items
        .iter()
        .find(|item| item.item_key == owner)
        .unwrap();
    assert_eq!(answer.kind, ChatPresentedItemKind::Answer);
    assert_ne!(answer.evidence_version, initial_version);
    let version = answer.evidence_version.clone();
    let order = answer.order_key.clone();
    let restored: Index = serde_json::from_slice(&snapshot).unwrap();
    let mut refresh = mirrors[0].clone();
    refresh.id = certificate.id;
    let mut reversed = certificate.members;
    reversed.reverse();
    refresh.metadata["chat_display_member_ids"] = json!(reversed);
    let older = page(&restored, temp.path(), &[refresh.clone()], true);
    assert_eq!(older.items.len(), 1);
    assert_eq!(older.items[0].item_key, owner);
    assert_eq!(older.items[0].order_key, order);
    assert_eq!(older.items[0].kind, ChatPresentedItemKind::Answer);
    assert_eq!(older.items[0].evidence_version, version);
    assert_eq!(older.turns[0].answer_item_keys.len(), 1);
    assert!(older.turns[0].answer_recorded);
    assert_eq!(serde_json::to_vec(&restored).unwrap(), snapshot);

    // Replay the same positive logical certificate within one read operation.
    // Immutable values already hash-validated here need no second file read.
    let mut reader = Store::new(temp.path());
    let mut cache = SeekCache::default();
    let events = [refresh];
    let owners = restored
        .preview_owners_reusing_nodes(&mut reader, &scope, &events, None, &mut cache)
        .unwrap()
        .expect("the earliest physical mirror retains its certified answer owner");
    assert_eq!(owners.rows[&events[0].id].item.membership.item_key, owner);
    assert_eq!(
        owners.source,
        Some((
            scope.source_key.unwrap().to_owned(),
            scope.source_epoch.unwrap().to_owned(),
        ))
    );
    let objects = reader.objects;
    let bytes = reader.bytes;
    let replay = restored
        .preview_owners_reusing_nodes(&mut reader, &scope, &events, None, &mut cache)
        .unwrap()
        .expect("the identical positive certificate remains qualified");
    assert_eq!(
        serde_json::to_vec(&replay).unwrap(),
        serde_json::to_vec(&owners).unwrap()
    );
    assert_eq!(serde_json::to_vec(&restored).unwrap(), snapshot);
    assert!(reader.objects <= READ_OBJECTS && reader.bytes <= READ_BYTES);
    assert_eq!(
        reader.objects - objects,
        0,
        "replaying positive values must not reopen immutable objects (additional bytes: {})",
        reader.bytes - bytes
    );
}

#[test]
fn activation_requires_the_matching_codex_or_claude_source_adapter() {
    for (provider, adapter, eligible) in [
        ("codex", Some("codex_bracket_v1"), true),
        ("claude", Some("claude_parent_chain_v1"), true),
        ("codex", Some("claude_parent_chain_v1"), false),
        ("claude", Some("codex_bracket_v1"), false),
        ("codex", None, false),
        ("gemini", Some("codex_bracket_v1"), false),
        ("mock", Some("codex_bracket_v1"), false),
        ("unknown", Some("codex_bracket_v1"), false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut index = Index::default();
        let mut request = prompt("request", "root");
        request.provider = provider.into();
        request.metadata["chat_turn_adapter"] = json!(adapter);
        observe(&mut index, temp.path(), &request, "000");
        let result = index
            .page(&mut Store::new(temp.path()), &[request], true)
            .unwrap();
        assert_eq!(result.is_some(), eligible, "{provider} {adapter:?}");
        if let Some(value) = result {
            assert!(value.turns[0].membership_complete);
            assert!(value.turns[0].counts_complete);
        }
    }
}

#[test]
fn closure_ignores_late_broken_rows_without_changing_completed_membership() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let mut request = prompt("request", "root");
    request.metadata["chat_turn_membership_complete"] = json!(false);
    let mut call = tool("rooted-call", "root", Some("call"), false);
    call.metadata["chat_turn_membership_complete"] = json!(false);
    observe(&mut index, temp.path(), &request, "000");
    observe(&mut index, temp.path(), &call, "001");
    let open = page(&index, temp.path(), &[request.clone(), call.clone()], true);
    assert!(!open.turns[0].membership_complete);
    assert!(!open.turns[0].counts_complete);
    assert_eq!(open.turns[0].step_count, 1);
    let mut closure = event("closure", AgentChatEventKind::Status, Some("root"));
    closure.status = Some(AgentChatStatus::Succeeded);
    observe(&mut index, temp.path(), &closure, "002");
    let closed = page(&index, temp.path(), &[request.clone(), call.clone()], false);
    assert!(closed.turns[0].membership_complete);
    assert!(closed.turns[0].counts_complete);
    assert!(!closed.turns[0].references_complete);
    let mut broken = event("broken-proof", AgentChatEventKind::Status, Some("root"));
    broken.metadata["chat_turn_membership_complete"] = json!(false);
    broken.metadata["chat_turn_membership_partial"] = json!(true);
    let snapshot = serde_json::to_vec(&index).unwrap();
    assert!(observe(&mut index, temp.path(), &broken, "003").is_none());
    observe(&mut index, temp.path(), &closure, "999");
    assert_eq!(serde_json::to_vec(&index).unwrap(), snapshot);
    assert!(index
        .event_keys(&mut Store::new(temp.path()), &broken.id)
        .unwrap()
        .is_none());
    let restored: Index = serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
    let unlinked = tool("unlinked", "", Some("other-call"), false);
    let value = page(
        &restored,
        temp.path(),
        &[request, call, broken, unlinked],
        true,
    );
    assert!(value.turns[0].membership_complete);
    assert!(value.turns[0].counts_complete);
    assert_eq!(value.turns[0].status, ChatTurnStatus::Completed);
    assert_eq!(value.turns[0].step_count, 1);
    assert_eq!(
        value.legacy_turns[0].event_refs,
        ["broken-proof", "unlinked"]
    );
}

#[test]
fn broken_proof_before_closure_remains_incomplete_after_terminal_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let mut request = prompt("request", "root");
    request.metadata["chat_turn_membership_complete"] = json!(false);
    let mut call = tool("rooted-call", "root", Some("call"), false);
    call.metadata["chat_turn_membership_complete"] = json!(false);
    observe(&mut index, temp.path(), &request, "000");
    observe(&mut index, temp.path(), &call, "001");
    let mut broken = event("broken-proof", AgentChatEventKind::Status, Some("root"));
    broken.metadata["chat_turn_membership_complete"] = json!(false);
    broken.metadata["chat_turn_membership_partial"] = json!(true);
    observe(&mut index, temp.path(), &broken, "002");
    let mut closure = event("closure", AgentChatEventKind::Status, Some("root"));
    closure.status = Some(AgentChatStatus::Succeeded);
    observe(&mut index, temp.path(), &closure, "003");
    let restored: Index = serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
    let unlinked = tool("unlinked", "", Some("other-call"), false);
    let partial = page(
        &restored,
        temp.path(),
        &[request, call, broken, closure, unlinked],
        true,
    );
    assert!(!partial.turns[0].membership_complete);
    assert!(!partial.turns[0].counts_complete);
    assert_eq!(partial.turns[0].step_count, 1);
    assert_eq!(partial.legacy_turns[0].event_refs, ["unlinked"]);
}

#[test]
fn stored_turns_without_positive_closure_default_to_partial() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    observe(&mut index, temp.path(), &request, "000");
    let (turn_key, _) = index
        .event_keys(&mut Store::new(temp.path()), &request.id)
        .unwrap()
        .unwrap();
    let mut store = Store::writer(temp.path());
    let reference = store.get(&index.turns, &turn_key).unwrap().unwrap();
    let mut old: serde_json::Value = store.read(&reference).unwrap();
    old.as_object_mut().unwrap().remove("membership_closed");
    old.as_object_mut().unwrap().remove("invalid_proof");
    let reference = store.put(&old).unwrap();
    index.turns = Some(store.insert(&index.turns, &turn_key, &reference).unwrap());
    let restored: Index = serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
    let value = page(&restored, temp.path(), &[request], true);
    assert!(!value.turns[0].membership_complete);
    assert!(!value.turns[0].counts_complete);
}

#[test]
fn an_unproven_record_cannot_contribute_to_an_adapter_qualified_turn() {
    let temp = tempfile::tempdir().unwrap();
    let mut index = Index::default();
    let request = prompt("request", "root");
    let mut unproven = tool("unproven", "root", Some("unproven-call"), false);
    unproven
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("chat_turn_adapter");
    let good = prompt("other-request", "other-root");
    for (i, event) in [&request, &unproven, &good].into_iter().enumerate() {
        observe(&mut index, temp.path(), event, &format!("{i:03}"));
    }
    let value = page(&index, temp.path(), &[request, unproven, good], true);
    assert_eq!(value.turns.len(), 1);
    assert_eq!(value.turns[0].step_count, 0);
    assert_eq!(value.legacy_turns.len(), 1);
    assert_eq!(value.legacy_turns[0].event_refs.len(), 2);
}
