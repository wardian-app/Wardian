use super::*;

fn context(turn: &str) -> Value {
    json!({"type": "turn_context", "payload": {"turn_id": turn}})
}

fn started(turn: &str) -> Value {
    json!({"type": "event_msg", "payload": {"type": "task_started", "turn_id": turn}})
}

fn complete(turn: &str) -> Value {
    json!({"type": "event_msg", "payload": {"type": "task_complete", "turn_id": turn}})
}

fn prompt(turn: &str) -> Value {
    json!({
        "type": "response_item",
        "payload": {
            "type": "message", "id": "native-message", "role": "user",
            "content": [{"type": "input_text", "text": "must-not-retain-prompt-text"}],
            "internal_chat_message_metadata_passthrough": {
                "turn_id": turn, "content_item_kinds": ["user.text"]
            }
        }
    })
}

fn call(call_id: &str) -> Value {
    json!({
        "type": "response_item",
        "payload": {
            "type": "function_call", "call_id": call_id, "name": "shell",
            "arguments": "must-not-retain-tool-input"
        }
    })
}

fn result(call_id: &str) -> Value {
    json!({
        "type": "response_item",
        "payload": {
            "type": "function_call_output", "call_id": call_id,
            "output": "must-not-retain-tool-output"
        }
    })
}

/// The parent owns composition with the real normalizer. These rows supply its
/// established classification/root contract without reproducing its parser.
fn row(kind: AgentChatEventKind, source: &str) -> AgentChatEvent {
    serde_json::from_value(json!({
        "id": "normalized-row", "session_id": "wardian-agent", "provider": "codex",
        "kind": kind, "source": source, "metadata": {}
    }))
    .expect("synthetic normalized event")
}

fn request_row(root: &str) -> AgentChatEvent {
    let mut event = row(AgentChatEventKind::Message, "response_item");
    event.role = Some(AgentChatRole::User);
    event.metadata = json!({
        "request_root_id": root,
        "input_origin": "human_input",
        "input_purpose": "request"
    });
    event
}

fn finish(state: &mut CodexState, turn: &str) -> AgentChatEvent {
    let mut event = row(AgentChatEventKind::Status, "task_complete");
    state.observe(&complete(turn), Some(&mut event));
    event
}

fn opened() -> CodexState {
    let mut state = CodexState::default();
    state.observe(&context("turn-a"), None);
    state.observe(&started("turn-a"), None);
    let mut request = request_row("request-a");
    state.observe(&prompt("turn-a"), Some(&mut request));
    assert_member(&request, "turn-a", "request-a");
    state
}

fn assert_member(event: &AgentChatEvent, turn: &str, root: &str) {
    assert_eq!(event.metadata["chat_turn_adapter"], ADAPTER);
    assert_eq!(event.metadata["provider_turn_id"], turn);
    assert_eq!(event.metadata["request_root_id"], root);
}

fn assert_unrooted(event: &AgentChatEvent) {
    // Existing legacy provenance is independent of the new proof capability.
    assert!(event.metadata.get("chat_turn_adapter").is_none());
    assert_eq!(event.metadata["chat_turn_membership_complete"], false);
}

#[test]
fn withholding_adapter_proof_preserves_legacy_identity_fields() {
    let mut state = CodexState::default();
    let raw = call("call");
    let mut event = crate::providers::chat_transcript::normalize_chat_line(
        "agent",
        "codex",
        &raw.to_string(),
        1,
    )
    .unwrap();
    event.metadata["request_root_id"] = json!("legacy-root");
    event.metadata["provider_turn_id"] = json!("legacy-turn");
    state.observe(&raw, Some(&mut event));
    assert_unrooted(&event);
    assert_eq!(event.metadata["request_root_id"], "legacy-root");
    assert_eq!(event.metadata["provider_turn_id"], "legacy-turn");
}

#[test]
fn real_rollout_shapes_require_typed_request_and_close_exact_bracket() {
    let raw: Vec<Value> = include_str!("../fixtures/codex-real-delivery-mirror.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("rollout record"))
        .collect();
    let turn = raw[0]["payload"]["turn_id"].as_str().expect("typed turn");
    let mut state = CodexState::default();
    // Both are observed even though the context normalizes to None.
    state.observe(&raw[0], None);
    state.observe(&raw[1], None);
    let mut request = request_row("real-request-root");
    state.observe(&raw[2], Some(&mut request));
    assert_member(&request, turn, "real-request-root");
    assert_eq!(request.metadata["chat_turn_membership_complete"], false);

    let mut mirror = request_row("real-request-root");
    mirror.source = Some("event_msg".into());
    mirror.metadata["provider_turn_id"] = json!(turn);
    state.observe(&raw[3], Some(&mut mirror));
    assert_member(&mirror, turn, "real-request-root");

    for record in &raw[4..] {
        let mut answer = row(
            AgentChatEventKind::Message,
            record["type"].as_str().unwrap(),
        );
        answer.role = Some(AgentChatRole::Assistant);
        state.observe(record, Some(&mut answer));
        assert_member(&answer, turn, "real-request-root");
        assert_eq!(answer.metadata["chat_turn_membership_complete"], false);
    }
    let terminal = finish(&mut state, turn);
    assert_member(&terminal, turn, "real-request-root");
    assert_eq!(terminal.metadata["chat_turn_membership_complete"], true);
    assert_eq!(terminal.metadata["chat_turn_membership_partial"], false);
    assert!(state.active.is_none());
    assert!(state.requests.is_empty());
}

#[test]
fn actual_prompt_before_start_is_tagged_without_granting_work_or_full_counts() {
    let raw: Vec<Value> = include_str!("../fixtures/codex-user-input-mirror.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("rollout record"))
        .collect();
    let turn = "provider-turn-a";
    let mut state = CodexState::default();
    state.observe(&raw[0], None);
    let mut request = request_row("request-a");
    state.observe(&raw[1], Some(&mut request));
    assert_member(&request, turn, "request-a");
    assert_eq!(request.metadata["chat_turn_membership_complete"], false);
    // A not-yet-started turn is not positively broken. This flag would poison
    // the producer's later exact closure if it were used for ordinary openness.
    assert_eq!(request.metadata["chat_turn_membership_partial"], false);
    let mut mirror = request_row("request-a");
    mirror.source = Some("event_msg".into());
    mirror.metadata["provider_turn_id"] = json!(turn);
    state.observe(&raw[2], Some(&mut mirror));
    assert_unrooted(&mirror);

    let mut premature = row(AgentChatEventKind::ToolCall, "response_item");
    state.observe(&call("before-start"), Some(&mut premature));
    assert_unrooted(&premature);
    let mut state: CodexState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    let mut start = row(AgentChatEventKind::ToolCall, "event_msg");
    state.observe(&started(turn), Some(&mut start));
    assert_member(&start, turn, "request-a");
    assert_eq!(start.metadata["chat_turn_membership_complete"], false);
    for (raw, kind) in [
        (call("call-a"), AgentChatEventKind::ToolCall),
        (result("call-a"), AgentChatEventKind::ToolResult),
    ] {
        let mut event = row(kind, "response_item");
        state.observe(&raw, Some(&mut event));
        assert_member(&event, turn, "request-a");
    }
    let mut narration = row(AgentChatEventKind::Message, "event_msg");
    narration.role = Some(AgentChatRole::Assistant);
    state.observe(&json!({"type":"event_msg","payload":{"type":"agent_message","message":"Unphased narration"}}), Some(&mut narration));
    assert_member(&narration, turn, "request-a");
    assert!(narration.metadata.get("provider_phase").is_none());
    let mut answer = row(AgentChatEventKind::Message, "response_item");
    answer.role = Some(AgentChatRole::Assistant);
    answer.metadata["provider_phase"] = json!("final_answer");
    state.observe(&json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","internal_chat_message_metadata_passthrough":{"turn_id":turn},"content":[{"type":"output_text","text":"Answer"}]}}), Some(&mut answer));
    assert_member(&answer, turn, "request-a");
    assert_eq!(answer.metadata["provider_phase"], "final_answer");
    assert_eq!(
        finish(&mut state, turn).metadata["chat_turn_membership_complete"],
        true
    );
}

#[test]
fn typed_prompt_can_precede_or_follow_context_and_start() {
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let mut state = CodexState::default();
        for operation in order {
            match operation {
                0 => state.observe(&context("turn-a"), None),
                1 => state.observe(&started("turn-a"), None),
                _ => {
                    let mut event = request_row("request-a");
                    state.observe(&prompt("turn-a"), Some(&mut event));
                    assert_eq!(event.metadata["request_root_id"], "request-a");
                    assert_eq!(event.metadata["chat_turn_membership_complete"], false);
                }
            }
        }
        let terminal = finish(&mut state, "turn-a");
        assert_member(&terminal, "turn-a", "request-a");
        assert_eq!(
            terminal.metadata["chat_turn_membership_complete"], true,
            "{order:?}"
        );
    }
}

#[test]
fn open_bracket_and_missing_boundary_never_certify_totals() {
    let mut state = opened();
    let mut work = row(AgentChatEventKind::ToolCall, "response_item");
    state.observe(&call("call-a"), Some(&mut work));
    assert_member(&work, "turn-a", "request-a");
    assert_eq!(work.metadata["chat_turn_membership_complete"], false);
    assert_eq!(work.metadata["chat_turn_membership_partial"], false);

    for missing_context in [true, false] {
        let mut state = CodexState::default();
        if missing_context {
            state.observe(&started("turn-a"), None);
        } else {
            state.observe(&context("turn-a"), None);
        }
        let mut request = request_row("request-a");
        state.observe(&prompt("turn-a"), Some(&mut request));
        let terminal = finish(&mut state, "turn-a");
        assert_unrooted(&terminal);
        assert_eq!(terminal.metadata["chat_turn_membership_partial"], true);
    }
}

#[test]
fn untyped_user_and_context_injection_cannot_supply_request_binding() {
    for untyped in [true, false] {
        let mut state = CodexState::default();
        state.observe(&context("turn-a"), None);
        state.observe(&started("turn-a"), None);
        let mut raw = prompt("turn-a");
        let mut request = request_row("inherited-neighbour-root");
        request.metadata["provider_turn_id"] = json!("turn-a");
        if untyped {
            raw["payload"]["internal_chat_message_metadata_passthrough"] = json!({});
        } else {
            request.metadata["input_origin"] = json!("context_injection");
            request.metadata["input_purpose"] = json!("context");
        }
        state.observe(&raw, Some(&mut request));
        assert_unrooted(&request);
        let mut work = row(AgentChatEventKind::ToolCall, "response_item");
        work.metadata["request_root_id"] = json!("inherited-neighbour-root");
        state.observe(&call("call-a"), Some(&mut work));
        assert_unrooted(&work);
        assert_unrooted(&finish(&mut state, "turn-a"));
    }
}

#[test]
fn parallel_call_and_result_identities_remain_distinct() {
    let mut state = opened();
    for (kind, raws) in [
        (
            AgentChatEventKind::ToolCall,
            [call("call-a"), call("call-b")],
        ),
        (
            AgentChatEventKind::ToolResult,
            [result("call-b"), result("call-a")],
        ),
    ] {
        for mut raw in raws {
            // The normalizer may put this provider turn in event.turn_id. The
            // adapter must retain the separate invocation identity as well.
            raw["payload"]["turn_id"] = json!("turn-a");
            let mut event = row(kind.clone(), "response_item");
            event.turn_id = Some("turn-a".into());
            state.observe(&raw, Some(&mut event));
            assert_member(&event, "turn-a", "request-a");
            assert_eq!(event.metadata["call_id"], raw["payload"]["call_id"]);
            assert_eq!(event.turn_id.as_deref(), Some("turn-a"));
        }
    }
    assert_eq!(
        finish(&mut state, "turn-a").metadata["chat_turn_membership_complete"],
        true
    );
}

#[test]
fn idless_calls_results_and_approval_inherit_only_active_explicit_bracket() {
    let mut state = opened();
    for (raw, kind, source) in [
        (
            call("call-a"),
            AgentChatEventKind::ToolCall,
            "response_item",
        ),
        (
            result("call-a"),
            AgentChatEventKind::ToolResult,
            "response_item",
        ),
        (
            json!({"type": "event_msg", "payload": {"type": "exec_approval_request", "call_id": "call-b"}}),
            AgentChatEventKind::Approval,
            "event_msg",
        ),
    ] {
        let mut event = row(kind, source);
        event.turn_id = Some("native-call-identity".into());
        state.observe(&raw, Some(&mut event));
        assert_member(&event, "turn-a", "request-a");
        assert_eq!(event.turn_id.as_deref(), Some("native-call-identity"));
        assert_eq!(event.metadata["chat_turn_membership_complete"], false);
    }
}

#[test]
fn conflicting_explicit_ids_are_unrooted_and_invalidate_closure_proof() {
    for explicit in [
        json!("turn-b"),
        json!(17),
        json!(null),
        json!(""),
        json!("x".repeat(MAX_ID_BYTES + 1)),
    ] {
        let mut state = opened();
        let mut raw = call("call-a");
        raw["payload"]["turn_id"] = explicit;
        let mut event = row(AgentChatEventKind::ToolCall, "response_item");
        event.metadata["request_root_id"] = json!("request-a");
        state.observe(&raw, Some(&mut event));
        assert_unrooted(&event);
        assert_eq!(event.metadata["chat_turn_membership_partial"], true);
        let terminal = finish(&mut state, "turn-a");
        assert_member(&terminal, "turn-a", "request-a");
        assert_eq!(terminal.metadata["chat_turn_membership_complete"], false);
        assert_eq!(terminal.metadata["chat_turn_membership_partial"], true);
    }
}

#[test]
fn multiple_typed_id_fields_must_agree_without_coercion() {
    for raw in [
        json!({"type":"event_msg","turn_id":"turn-a","payload":{"type":"agent_message","turn_id":"turn-b"}}),
        json!({"type":"response_item","turn_id":"turn-a","payload":{"type":"message","role":"assistant","internal_chat_message_metadata_passthrough":{"turn_id":"turn-b"}}}),
        json!({"type":"response_item","turn_id":17,"payload":{"type":"message","role":"assistant","turn_id":"turn-a"}}),
    ] {
        let mut state = opened();
        let mut event = row(AgentChatEventKind::Message, raw["type"].as_str().unwrap());
        event.role = Some(AgentChatRole::Assistant);
        state.observe(&raw, Some(&mut event));
        assert_unrooted(&event);
        assert_eq!(
            finish(&mut state, "turn-a").metadata["chat_turn_membership_complete"],
            false
        );
    }
}

#[test]
fn mismatched_completion_does_not_close_active_turn() {
    let mut state = opened();
    let other = finish(&mut state, "turn-b");
    assert_unrooted(&other);
    assert_eq!(other.metadata["chat_turn_membership_partial"], true);
    assert_eq!(state.active.as_ref().unwrap().turn_id, "turn-a");
    let mut work = row(AgentChatEventKind::ToolResult, "response_item");
    state.observe(&result("call-a"), Some(&mut work));
    assert_member(&work, "turn-a", "request-a");
    assert_eq!(work.metadata["chat_turn_membership_partial"], true);
    assert_eq!(
        finish(&mut state, "turn-a").metadata["chat_turn_membership_complete"],
        false
    );
}

#[test]
fn context_switch_and_unrepresented_work_break_full_proof() {
    let mut state = opened();
    state.observe(&context("turn-b"), None);
    let mut work = row(AgentChatEventKind::ToolCall, "response_item");
    state.observe(&call("call-a"), Some(&mut work));
    assert_unrooted(&work);
    state.observe(&context("turn-a"), None);
    let terminal = finish(&mut state, "turn-a");
    assert_member(&terminal, "turn-a", "request-a");
    assert_eq!(terminal.metadata["chat_turn_membership_complete"], false);
    assert_eq!(terminal.metadata["chat_turn_membership_partial"], true);

    let mut state = opened();
    state.observe(&call("call-not-normalized"), None);
    assert_eq!(
        finish(&mut state, "turn-a").metadata["chat_turn_membership_complete"],
        false
    );
}

#[test]
fn neighbouring_events_after_closure_and_unknown_shapes_stay_unrooted() {
    let mut state = opened();
    assert_eq!(
        finish(&mut state, "turn-a").metadata["chat_turn_membership_complete"],
        true
    );
    let raw = json!({"type":"event_msg","payload":{"type":"agent_message","message":"same text"}});
    let mut neighbour = row(AgentChatEventKind::Message, "event_msg");
    neighbour.role = Some(AgentChatRole::Assistant);
    neighbour.metadata = json!({"request_root_id":"request-a", "provider_turn_id":"turn-a", "chat_turn_adapter":ADAPTER, "chat_turn_membership_complete":true});
    state.observe(&raw, Some(&mut neighbour));
    assert_unrooted(&neighbour);

    let mut state = opened();
    let mut unsupported = row(AgentChatEventKind::ToolCall, "unknown-wrapper");
    unsupported.metadata["request_root_id"] = json!("request-a");
    state.observe(
        &json!({"type":"unknown-wrapper","payload":{"type":"function_call"}}),
        Some(&mut unsupported),
    );
    assert_unrooted(&unsupported);
}

#[test]
fn preserve_matching_root_but_never_overwrite_conflicting_root_or_provider() {
    let mut state = opened();
    let mut matching = row(AgentChatEventKind::ToolCall, "response_item");
    matching.metadata["request_root_id"] = json!("request-a");
    state.observe(&call("call-a"), Some(&mut matching));
    assert_member(&matching, "turn-a", "request-a");

    for foreign in ["root", "provider", "agent", "call", "turn"] {
        let mut state = opened();
        let mut event = row(AgentChatEventKind::ToolCall, "response_item");
        match foreign {
            "root" => event.metadata["request_root_id"] = json!("another-root"),
            "provider" => event.provider = "claude".into(),
            "agent" => event.session_id = "another-agent".into(),
            "turn" => event.metadata["provider_turn_id"] = json!("turn-b"),
            _ => event.metadata["call_id"] = json!("another-call"),
        }
        state.observe(&call("call-a"), Some(&mut event));
        assert_unrooted(&event);
        assert_eq!(event.metadata["chat_turn_membership_partial"], true);
        assert_eq!(
            finish(&mut state, "turn-a").metadata["chat_turn_membership_complete"],
            false
        );
    }
}

#[test]
fn conflicting_requests_for_one_typed_turn_never_select_a_neighbour_root() {
    let mut state = opened();
    let mut conflicting = request_row("request-b");
    state.observe(&prompt("turn-a"), Some(&mut conflicting));
    assert_unrooted(&conflicting);
    let mut work = row(AgentChatEventKind::ToolCall, "response_item");
    state.observe(&call("call-a"), Some(&mut work));
    assert_unrooted(&work);
    let terminal = finish(&mut state, "turn-a");
    assert_unrooted(&terminal);
    assert_eq!(terminal.metadata["chat_turn_membership_partial"], true);
}

#[test]
fn serde_continuation_preserves_live_and_broken_proof_without_raw_content() {
    assert_eq!(
        serde_json::from_value::<CodexState>(json!({})).unwrap(),
        CodexState::default()
    );
    let mut state = opened();
    let mut work = row(AgentChatEventKind::ToolCall, "response_item");
    work.text = Some("must-not-retain-normalized-text".into());
    state.observe(&call("call-a"), Some(&mut work));
    let saved = serde_json::to_string(&state).unwrap();
    assert!(!saved.contains("must-not-retain"));
    let mut resumed: CodexState = serde_json::from_str(&saved).unwrap();
    assert_eq!(resumed, state);
    let mut output = row(AgentChatEventKind::ToolResult, "response_item");
    resumed.observe(&result("call-a"), Some(&mut output));
    assert_member(&output, "turn-a", "request-a");
    assert_eq!(
        finish(&mut resumed, "turn-a").metadata["chat_turn_membership_complete"],
        true
    );

    finish(&mut state, "turn-b");
    let mut resumed: CodexState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    let terminal = finish(&mut resumed, "turn-a");
    assert_eq!(terminal.metadata["chat_turn_membership_complete"], false);
    assert_eq!(terminal.metadata["chat_turn_membership_partial"], true);
}

#[test]
fn session_meta_rollover_forgets_old_request_even_when_turn_id_repeats() {
    let header = |session: &str| json!({"type":"session_meta","payload":{"id":session}});
    let mut state = CodexState::default();
    state.observe(&header("session-a"), None);
    state.observe(&context("turn-a"), None);
    state.observe(&started("turn-a"), None);
    state.observe(&prompt("turn-a"), Some(&mut request_row("old-root")));
    let before = state.clone();
    state.observe(&header("session-a"), None);
    assert_eq!(state, before);
    state.observe(&header("session-b"), None);
    state.observe(&context("turn-a"), None);
    state.observe(&started("turn-a"), None);
    let mut work = row(AgentChatEventKind::ToolCall, "response_item");
    work.metadata["request_root_id"] = json!("old-root");
    state.observe(&call("call-a"), Some(&mut work));
    assert_unrooted(&work);
    assert_unrooted(&finish(&mut state, "turn-a"));
    state.observe(&context("turn-a"), None);
    state.observe(&started("turn-a"), None);
    let mut request = request_row("new-root");
    state.observe(&prompt("turn-a"), Some(&mut request));
    assert_member(&request, "turn-a", "new-root");
    assert_eq!(
        finish(&mut state, "turn-a").metadata["chat_turn_membership_complete"],
        true
    );
}

#[test]
fn identifier_and_pending_mapping_limits_fail_closed_without_truncation() {
    let mut state = CodexState::default();
    let exact = "x".repeat(MAX_ID_BYTES);
    state.observe(&context(&exact), None);
    state.observe(&started(&exact), None);
    state.observe(&prompt(&exact), Some(&mut request_row(&exact)));
    assert_eq!(
        finish(&mut state, &exact).metadata["chat_turn_membership_complete"],
        true
    );
    let too_long = "é".repeat(MAX_ID_BYTES);
    state.observe(&context(&too_long), None);
    state.observe(&started(&too_long), None);
    let mut event = request_row("request-a");
    state.observe(&prompt(&too_long), Some(&mut event));
    assert_unrooted(&event);
    assert!(state.valid());

    let mut state = CodexState::default();
    for index in 0..=MAX_REQUESTS {
        let turn = format!("pending-{index}");
        state.observe(&prompt(&turn), Some(&mut request_row("pending-root")));
        assert!(state.requests.len() <= MAX_REQUESTS);
    }
    assert!(state.blocked);
    state.observe(&context("pending-0"), None);
    state.observe(&started("pending-0"), None);
    assert_unrooted(&finish(&mut state, "pending-0"));
}

#[test]
fn completed_turns_release_capacity_and_invalid_continuation_is_not_trusted() {
    let mut state = CodexState::default();
    for index in 0..=MAX_REQUESTS {
        let turn = format!("finished-{index}");
        state.observe(&context(&turn), None);
        state.observe(&started(&turn), None);
        state.observe(&prompt(&turn), Some(&mut request_row("root")));
        assert_eq!(
            finish(&mut state, &turn).metadata["chat_turn_membership_complete"],
            true
        );
        assert!(state.requests.is_empty());
    }
    assert!(!state.blocked);
    let mut saved = serde_json::to_value(opened()).unwrap();
    saved["active"]["turn_id"] = json!("x".repeat(MAX_ID_BYTES + 1));
    let mut restored: CodexState = serde_json::from_value(saved).unwrap();
    let mut event = row(AgentChatEventKind::ToolResult, "response_item");
    restored.observe(&result("call-a"), Some(&mut event));
    assert_unrooted(&event);
    assert!(restored.blocked);
    assert!(restored.valid());
}

#[test]
fn stored_identity_byte_limit_and_malformed_session_fail_closed_until_rollover() {
    let mut state = CodexState::default();
    for index in 0..MAX_REQUESTS {
        let turn = format!("{index:08}{}", "x".repeat(MAX_ID_BYTES - 8));
        state.observe(
            &prompt(&turn),
            Some(&mut request_row(&"r".repeat(MAX_ID_BYTES))),
        );
        assert!(state.stored_id_bytes() <= MAX_STORED_ID_BYTES);
        assert!(state.valid());
        if state.blocked {
            break;
        }
    }
    assert!(state.blocked);
    assert!(state.requests.is_empty());
    state.observe(
        &json!({"type":"session_meta","payload":{"id":"fresh-native-session"}}),
        None,
    );
    assert!(!state.blocked);
    state.observe(&context("turn-a"), None);
    state.observe(&started("turn-a"), None);
    let mut request = request_row("request-a");
    state.observe(&prompt("turn-a"), Some(&mut request));
    assert_member(&request, "turn-a", "request-a");
    state.observe(&json!({"type":"session_meta","payload":{"id":17}}), None);
    let mut event = row(AgentChatEventKind::ToolCall, "response_item");
    state.observe(&call("call-a"), Some(&mut event));
    assert_unrooted(&event);
    assert!(state.blocked);
}

#[test]
fn new_unclosed_turn_never_borrows_old_request_and_agent_input_can_bind() {
    let mut state = opened();
    state.observe(&context("turn-b"), None);
    state.observe(&started("turn-b"), None);
    let mut work = row(AgentChatEventKind::ToolCall, "response_item");
    work.metadata["request_root_id"] = json!("request-a");
    state.observe(&call("call-b"), Some(&mut work));
    assert_unrooted(&work);
    let mut request = request_row("agent-request-b");
    request.metadata["input_origin"] = json!("agent_input");
    state.observe(&prompt("turn-b"), Some(&mut request));
    assert_member(&request, "turn-b", "agent-request-b");
    assert_eq!(request.metadata["chat_turn_membership_partial"], true);
    assert_eq!(
        finish(&mut state, "turn-b").metadata["chat_turn_membership_complete"],
        false
    );
}

#[test]
fn work_before_context_and_untyped_completion_cannot_be_repaired_into_full_proof() {
    let mut state = CodexState::default();
    state.observe(&started("turn-a"), None);
    state.observe(&prompt("turn-a"), Some(&mut request_row("request-a")));
    let mut work = row(AgentChatEventKind::ToolCall, "response_item");
    state.observe(&call("call-a"), Some(&mut work));
    assert_unrooted(&work);
    state.observe(&context("turn-a"), None);
    let terminal = finish(&mut state, "turn-a");
    assert_member(&terminal, "turn-a", "request-a");
    assert_eq!(terminal.metadata["chat_turn_membership_complete"], false);

    let mut state = opened();
    let mut untyped = row(AgentChatEventKind::Status, "task_complete");
    state.observe(
        &json!({"type":"event_msg","payload":{"type":"task_complete"}}),
        Some(&mut untyped),
    );
    assert_unrooted(&untyped);
    assert!(state.active.is_some());
    assert_eq!(
        finish(&mut state, "turn-a").metadata["chat_turn_membership_complete"],
        false
    );
}
