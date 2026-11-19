use crate::providers::chat_transcript::{
    normalize_chat_lines, normalize_chat_lines_with_state, TranscriptNormalizationState,
};

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
}
