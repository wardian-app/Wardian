use crate::providers::chat_transcript::{
    normalize_chat_lines, normalize_chat_lines_with_state, TranscriptNormalizationState,
};
use serde_json::{json, Value};
use wardian_core::models::chat::AgentChatEvent;

fn assert_no_contradictions(events: &[AgentChatEvent]) {
    for event in events {
        for field in [
            "chat_turn_membership_contradiction",
            "chat_turn_membership_contradicted_root",
            "chat_turn_membership_expected_provider_turn",
        ] {
            assert!(event.metadata.get(field).is_none(), "unexpected {field}");
        }
    }
}

const PREFIX: [&str; 3] = [
    r#"{"type":"user","uuid":"request","sessionId":"synthetic-session","parentUuid":null,"message":{"role":"user","content":"Run synthetic tests."}}"#,
    r#"{"type":"assistant","uuid":"call","sessionId":"synthetic-session","parentUuid":"request","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"echo synthetic"}}],"stop_reason":"tool_use"}}"#,
    r#"{"type":"user","uuid":"result","sessionId":"synthetic-session","parentUuid":"call","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"passed"}]}}"#,
];
const FINAL: &str = r#"{"type":"assistant","uuid":"answer","sessionId":"synthetic-session","parentUuid":"result","message":{"role":"assistant","content":[{"type":"text","text":"The synthetic tests passed."}],"stop_reason":"end_turn"}}"#;

#[test]
fn parent_chain_proves_live_partial_then_closed_membership() {
    let live = normalize_chat_lines("synthetic-agent", "claude", PREFIX);
    assert_eq!(live.len(), 3);
    for event in &live {
        assert_eq!(event.metadata["request_root_id"], "request");
        assert_eq!(
            event.metadata["chat_turn_adapter"],
            "claude_parent_chain_v1"
        );
        assert_eq!(event.metadata["chat_turn_membership_complete"], false);
    }
    let events = normalize_chat_lines(
        "synthetic-agent",
        "claude",
        PREFIX.into_iter().chain([FINAL]),
    );
    let answer = events.last().unwrap();
    assert_eq!(answer.metadata["request_root_id"], "request");
    assert_eq!(answer.metadata["provider_phase"], "final_answer");
    assert_eq!(answer.metadata["chat_turn_membership_complete"], true);
    assert_no_contradictions(&events);
}

#[test]
fn sidechain_requires_an_explicit_parent_tool_and_neighbouring_broken_rows_stay_unrooted() {
    let sidechain = r#"{"type":"assistant","uuid":"side","parentUuid":"call","sessionId":"synthetic-session","isSidechain":true,"parent_tool_use_id":"tool-1","message":{"role":"assistant","content":[{"type":"text","text":"Side work."}]}}"#;
    let unlinked_sidechain = r#"{"type":"assistant","uuid":"unlinked-side","parentUuid":"call","sessionId":"synthetic-session","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"Side work."}]}}"#;
    let broken = r#"{"type":"assistant","uuid":"broken","parentUuid":"missing","sessionId":"synthetic-session","message":{"role":"assistant","content":[{"type":"text","text":"The synthetic tests passed."}]}}"#;
    let events = normalize_chat_lines(
        "synthetic-agent",
        "claude",
        PREFIX
            .into_iter()
            .chain([sidechain, unlinked_sidechain, broken, FINAL]),
    );
    assert_eq!(events[3].metadata["request_root_id"], "request");
    assert!(events[4].metadata.get("chat_turn_adapter").is_none());
    assert!(events[5].metadata.get("chat_turn_adapter").is_none());
    let answer = events.last().unwrap();
    assert_eq!(answer.metadata["chat_turn_membership_complete"], false);
    assert_eq!(answer.metadata["chat_turn_membership_partial"], true);
    assert_no_contradictions(&events);
}

#[test]
fn persisted_continuation_retains_links_without_text_and_session_rollover_retires_them() {
    let mut state = TranscriptNormalizationState::default();
    normalize_chat_lines_with_state("synthetic-agent", "claude", PREFIX, &mut state, false, true)
        .unwrap();
    let bytes = serde_json::to_vec(&state).unwrap();
    let encoded = String::from_utf8(bytes.clone()).unwrap();
    assert!(!encoded.contains("Run synthetic tests."));
    let mut restored: TranscriptNormalizationState = serde_json::from_slice(&bytes).unwrap();
    let answer = normalize_chat_lines_with_state(
        "synthetic-agent",
        "claude",
        [FINAL],
        &mut restored,
        false,
        true,
    )
    .unwrap();
    assert_eq!(answer[0].metadata["chat_turn_membership_complete"], true);
    let foreign = FINAL.replace("synthetic-session", "foreign-session");
    let answer = normalize_chat_lines_with_state(
        "synthetic-agent",
        "claude",
        [foreign],
        &mut restored,
        false,
        true,
    )
    .unwrap();
    assert!(answer[0].metadata.get("chat_turn_adapter").is_none());
}

#[test]
fn several_invocations_in_one_legacy_row_cannot_claim_complete_counts() {
    let multiple = r#"{"type":"assistant","uuid":"multiple","sessionId":"synthetic-session","parentUuid":"request","message":{"role":"assistant","content":[{"type":"tool_use","id":"one","name":"Read","input":{}},{"type":"tool_use","id":"two","name":"Read","input":{}}],"stop_reason":"end_turn"}}"#;
    let events = normalize_chat_lines("synthetic-agent", "claude", [PREFIX[0], multiple]);
    assert_eq!(
        events.last().unwrap().metadata["chat_turn_membership_complete"],
        false
    );
    assert_eq!(
        events.last().unwrap().metadata["chat_turn_membership_partial"],
        true
    );
}

#[test]
fn combined_native_results_with_one_normalized_result_keep_closure_partial() {
    let fixture = include_str!("../fixtures/claude-parent-chain-combined-results.jsonl");
    let events = normalize_chat_lines("synthetic-agent", "claude", fixture.lines());
    assert_eq!(events.iter().filter(|event| event.kind == wardian_core::models::chat::AgentChatEventKind::ToolResult).count(), 1);
    let answer = events.last().unwrap();
    assert_eq!(answer.metadata["request_root_id"], "request");
    assert_eq!(answer.metadata["chat_turn_membership_complete"], false);
    assert_eq!(answer.metadata["chat_turn_membership_partial"], true);
    assert_no_contradictions(&events);
}

#[test]
fn rewriting_a_known_uuid_edge_reports_the_exact_previous_root_without_payload() {
    for parent in [
        json!("result"),
        json!("previously-unseen-parent"),
        Value::Null,
    ] {
        let mut rewritten: Value = serde_json::from_str(PREFIX[1]).unwrap();
        rewritten["parentUuid"] = parent;
        rewritten["message"]["content"][0]["input"]["command"] = json!("private-command-payload");
        let events = normalize_chat_lines(
            "synthetic-agent",
            "claude",
            PREFIX
                .into_iter()
                .map(str::to_owned)
                .chain([rewritten.to_string()]),
        );
        let event = events.last().unwrap();
        assert_eq!(
            event.metadata["chat_turn_membership_contradiction"],
            "claude_uuid_ancestry_conflict"
        );
        assert_eq!(
            event.metadata["chat_turn_membership_contradicted_root"],
            "request"
        );
        assert!(event
            .metadata
            .get("chat_turn_membership_expected_provider_turn")
            .is_none());
        assert!(event.metadata.get("chat_turn_adapter").is_none());
        let reason = event.metadata["chat_turn_membership_contradiction"]
            .as_str()
            .unwrap();
        assert!(!reason.contains("private-command-payload"));
        assert!(!reason.contains("previously-unseen-parent"));
    }
}

#[test]
fn persisted_uuid_edges_detect_rewrites_but_legacy_edges_do_not_invent_evidence() {
    let mut state = TranscriptNormalizationState::default();
    normalize_chat_lines_with_state("synthetic-agent", "claude", PREFIX, &mut state, false, true)
        .unwrap();
    let serialized = serde_json::to_string(&state).unwrap();
    assert!(!serialized.contains("echo synthetic"));
    assert!(!serialized.contains("Run synthetic tests."));
    let mut rewritten: Value = serde_json::from_str(PREFIX[1]).unwrap();
    rewritten["parentUuid"] = json!("missing-parent");
    let mut restored = serde_json::from_str(&serialized).unwrap();
    let events = normalize_chat_lines_with_state(
        "synthetic-agent",
        "claude",
        [rewritten.to_string()],
        &mut restored,
        false,
        true,
    )
    .unwrap();
    assert_eq!(
        events.last().unwrap().metadata["chat_turn_membership_contradicted_root"],
        "request"
    );

    let mut legacy: Value = serde_json::from_str(&serialized).unwrap();
    for link in legacy["turn_membership"]["claude"]["links"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        link.as_object_mut().unwrap().remove("parent_uuid");
        link.as_object_mut().unwrap().remove("parent_observed");
    }
    let mut legacy_state = serde_json::from_value(legacy).unwrap();
    let events = normalize_chat_lines_with_state(
        "synthetic-agent",
        "claude",
        [rewritten.to_string()],
        &mut legacy_state,
        false,
        true,
    )
    .unwrap();
    assert_no_contradictions(&events);
}

#[test]
fn unknown_parent_truncated_suffix_compaction_and_subagents_are_not_rewrites() {
    // Synthetic shapes: the corpus has parent-chain and rendering fixtures but
    // no captured compaction or subagent transcript with complete UUID edges.
    let suffix = [
        json!({"type":"assistant","uuid":"unknown","parentUuid":"missing","sessionId":"synthetic-session","message":{"role":"assistant","content":[{"type":"text","text":"Unknown ancestry."}]}}),
        json!({"type":"assistant","uuid":"side","parentUuid":"call","sessionId":"synthetic-session","isSidechain":true,"parent_tool_use_id":"tool-1","message":{"role":"assistant","content":[{"type":"text","text":"Linked subagent work."}]}}),
        json!({"type":"assistant","uuid":"unlinked-side","parentUuid":"missing","sessionId":"synthetic-session","isSidechain":true,"agentId":"synthetic-subagent","message":{"role":"assistant","content":[{"type":"text","text":"Unlinked subagent work."}]}}),
        json!({"type":"system","subtype":"compact_boundary","uuid":"compact","parentUuid":null,"sessionId":"synthetic-session","compactMetadata":{"trigger":"auto","preTokens":100}}),
        json!({"type":"user","uuid":"summary","parentUuid":"compact","isCompactSummary":true,"sessionId":"synthetic-session","message":{"role":"user","content":"Synthetic compacted context."}}),
    ];
    for raw in &suffix {
        let events = normalize_chat_lines(
            "synthetic-agent",
            "claude",
            PREFIX
                .into_iter()
                .map(str::to_owned)
                .chain([raw.to_string()]),
        );
        assert_no_contradictions(&events);
    }
    let truncated: Vec<_> = PREFIX[1..].iter().copied().chain([FINAL]).collect();
    assert_no_contradictions(&normalize_chat_lines(
        "synthetic-agent",
        "claude",
        truncated,
    ));
    let mut omitted: Value = serde_json::from_str(PREFIX[1]).unwrap();
    omitted.as_object_mut().unwrap().remove("parentUuid");
    assert_no_contradictions(&normalize_chat_lines(
        "synthetic-agent",
        "claude",
        PREFIX
            .into_iter()
            .map(str::to_owned)
            .chain([omitted.to_string()]),
    ));
}

#[test]
fn early_unpaired_parallel_and_replayed_work_does_not_retract_membership() {
    let rows = [
        json!({"type":"user","uuid":"early","parentUuid":"request","sessionId":"synthetic-session","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"early-tool","content":"Early result."}]}}),
        json!({"type":"user","uuid":"unpaired","parentUuid":"early","sessionId":"synthetic-session","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"unpaired-tool","content":"Unpaired result."}]}}),
        serde_json::from_str(PREFIX[1]).unwrap(),
        json!({"type":"assistant","uuid":"parallel","parentUuid":"request","sessionId":"synthetic-session","message":{"role":"assistant","content":[{"type":"tool_use","id":"parallel-tool","name":"Read","input":{}}],"stop_reason":"tool_use"}}),
        serde_json::from_str(PREFIX[2]).unwrap(),
        serde_json::from_str(PREFIX[1]).unwrap(),
        serde_json::from_str(PREFIX[2]).unwrap(),
        serde_json::from_str(FINAL).unwrap(),
    ];
    let events = normalize_chat_lines(
        "synthetic-agent",
        "claude",
        [PREFIX[0].to_owned()]
            .into_iter()
            .chain(rows.iter().map(Value::to_string)),
    );
    assert_no_contradictions(&events);
    for event in &events {
        assert_eq!(event.metadata["chat_turn_membership_partial"], false);
        assert_eq!(event.metadata["request_root_id"], "request");
    }
    assert_eq!(
        events.last().unwrap().metadata["chat_turn_membership_complete"],
        true
    );
}

#[test]
fn closed_turns_and_new_prompts_never_retract_the_previous_chain() {
    let mut rewritten: Value = serde_json::from_str(PREFIX[1]).unwrap();
    rewritten["parentUuid"] = json!("missing");
    let events = normalize_chat_lines(
        "synthetic-agent",
        "claude",
        PREFIX
            .into_iter()
            .chain([FINAL])
            .map(str::to_owned)
            .chain([rewritten.to_string()]),
    );
    assert_no_contradictions(&events);
    assert_eq!(events[3].metadata["chat_turn_membership_complete"], true);

    let next = PREFIX[0].replace("\"request\"", "\"next-request\"");
    let events = normalize_chat_lines(
        "synthetic-agent",
        "claude",
        PREFIX
            .into_iter()
            .map(str::to_owned)
            .chain([next, rewritten.to_string()]),
    );
    assert_no_contradictions(&events);
}

#[test]
fn rendering_corpus_contains_no_contradiction_evidence() {
    let fixture = include_str!("../fixtures/claude-chat-render.jsonl");
    let events = normalize_chat_lines("synthetic-agent", "claude", fixture.lines());
    assert!(!events.is_empty());
    assert_no_contradictions(&events);
}

#[test]
fn replay_at_identity_capacity_is_not_new_work_or_a_membership_gap() {
    let mut events = normalize_chat_lines("synthetic-agent", "claude", PREFIX);
    let mut state = super::ClaudeState::default();
    for (raw, event) in PREFIX.into_iter().zip(&mut events) {
        state.observe(&serde_json::from_str(raw).unwrap(), Some(event));
    }
    let link = state.links["call"].clone();
    for index in state.links.len()..super::MAX_LINKS {
        state
            .links
            .insert(format!("other-uuid-{index}"), link.clone());
    }
    for index in state.tools.len()..super::MAX_LINKS {
        state
            .tools
            .insert(format!("other-tool-{index}"), "request".into());
    }
    let mut replay = events[1].clone();
    state.observe(&serde_json::from_str(PREFIX[1]).unwrap(), Some(&mut replay));
    assert_no_contradictions(&[replay.clone()]);
    assert_eq!(replay.metadata["chat_turn_membership_partial"], false);
    assert!(!state.saturated);
    assert_eq!(state.links.len(), super::MAX_LINKS);
    assert_eq!(state.tools.len(), super::MAX_LINKS);
}

#[test]
fn contradiction_without_a_visible_row_survives_continuation_until_the_next_event() {
    let mut normalizer = TranscriptNormalizationState::default();
    normalize_chat_lines_with_state(
        "synthetic-agent",
        "claude",
        PREFIX,
        &mut normalizer,
        false,
        true,
    )
    .unwrap();
    let saved = serde_json::to_value(&normalizer).unwrap();
    let mut state: super::ClaudeState =
        serde_json::from_value(saved["turn_membership"]["claude"].clone()).unwrap();
    let mut rewritten: Value = serde_json::from_str(PREFIX[1]).unwrap();
    rewritten["parentUuid"] = json!("missing-parent");
    state.observe(&rewritten, None);
    let serialized = serde_json::to_string(&state).unwrap();
    assert!(!serialized.contains("echo synthetic"));
    let mut restored: super::ClaudeState = serde_json::from_str(&serialized).unwrap();
    let mut answer = normalize_chat_lines("synthetic-agent", "claude", [FINAL])
        .pop()
        .unwrap();
    restored.observe(&serde_json::from_str(FINAL).unwrap(), Some(&mut answer));
    assert_eq!(
        answer.metadata["chat_turn_membership_contradiction"],
        "claude_uuid_ancestry_conflict"
    );
    assert_eq!(
        answer.metadata["chat_turn_membership_contradicted_root"],
        "request"
    );
    assert_eq!(answer.metadata["chat_turn_membership_complete"], false);
}

#[test]
fn closed_chain_late_wrapper_and_unpaired_result_are_flagged_without_retraction() {
    let mut state = TranscriptNormalizationState::default();
    let closed = normalize_chat_lines_with_state(
        "synthetic-agent",
        "claude",
        PREFIX.into_iter().chain([FINAL]),
        &mut state,
        false,
        true,
    )
    .unwrap();
    let answer = closed.last().unwrap();
    assert_eq!(answer.metadata["chat_turn_membership_complete"], true);
    assert!(answer.metadata.get("chat_turn_after_closed").is_none());
    let saved = serde_json::to_string(&state).unwrap();
    let mut restored = serde_json::from_str(&saved).unwrap();
    let result = r#"{"type":"user","uuid":"late-unpaired","sessionId":"synthetic-session","parentUuid":"answer","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"unpaired-tool","content":"late result"}]}}"#;
    let later = normalize_chat_lines_with_state(
        "synthetic-agent",
        "claude",
        [FINAL, result],
        &mut restored,
        false,
        true,
    )
    .unwrap();
    assert_eq!(later.len(), 2);
    assert_no_contradictions(&later);
    for event in &later {
        assert_eq!(event.metadata["chat_turn_after_closed"], true);
        assert_eq!(event.metadata["chat_turn_membership_complete"], false);
        assert_eq!(event.metadata["chat_turn_membership_partial"], false);
        assert!(event.metadata.get("chat_turn_adapter").is_none());
    }
    assert_eq!(answer.metadata["chat_turn_membership_complete"], true);
    let next = PREFIX[0].replace("\"request\"", "\"next-request\"");
    let next_events = normalize_chat_lines_with_state(
        "synthetic-agent",
        "claude",
        [next],
        &mut restored,
        false,
        true,
    )
    .unwrap();
    assert!(next_events[0]
        .metadata
        .get("chat_turn_after_closed")
        .is_none());
    assert_no_contradictions(&next_events);
    let open = normalize_chat_lines("synthetic-agent", "claude", PREFIX);
    assert!(open
        .iter()
        .all(|event| event.metadata.get("chat_turn_after_closed").is_none()));
}
