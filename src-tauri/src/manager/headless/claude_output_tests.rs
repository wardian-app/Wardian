//! Minimal verbose-output fixtures; no provider, credentials or real transcript.
use super::normalize_claude_headless_output;
use serde_json::{json, Value};

fn verbose(answer: &str) -> Value {
    json!([
        {"type":"system","subtype":"init","session_id":"fixture-session"},
        {"type":"assistant","message":{"content":[{"type":"text","text":"intermediate text"}]}},
        {"type":"result","subtype":"success","is_error":false,
         "session_id":"fixture-session","result":answer}
    ])
}

#[test]
fn claude_verbose_headless_result_preserves_answer_session_and_diagnostics() {
    let raw = verbose("  FINAL|current-marker\n").to_string();
    let output = normalize_claude_headless_output(&raw, "json").unwrap();
    assert_eq!(output["response"], "FINAL|current-marker");
    assert_eq!(output["session_id"], "fixture-session");
    assert_eq!(output["raw"], raw);
    assert!(!output["response"]
        .as_str()
        .unwrap()
        .contains("intermediate"));
    assert_eq!(
        normalize_claude_headless_output(&raw, "text").unwrap(),
        json!({"text":"FINAL|current-marker"})
    );
}

#[test]
fn claude_verbose_headless_empty_answer_does_not_fall_back_to_events() {
    let output = normalize_claude_headless_output(&verbose("").to_string(), "json").unwrap();
    assert_eq!(output["response"], "");
    assert_eq!(output["session_id"], "fixture-session");
}

#[test]
fn claude_verbose_headless_json_answer_remains_the_model_answer() {
    let answer = r#"{"answer":42}"#;
    let output = normalize_claude_headless_output(&verbose(answer).to_string(), "json").unwrap();
    assert_eq!(output["response"], answer);
}

#[test]
fn claude_verbose_headless_rejects_missing_ambiguous_and_error_results() {
    let terminal = json!({"type":"result","result":"PRIVATE_SENTINEL"});
    let invalid = [
        json!([]),
        json!([{"type":"assistant","message":{"content":"PRIVATE_SENTINEL"}}]),
        json!([terminal.clone(), terminal.clone()]),
        json!([terminal, {"type":"assistant","message":{"content":"later text"}}]),
        json!([{"type":"result","result":{"text":"PRIVATE_SENTINEL"}}]),
        json!([{"type":"result","is_error":true,"result":"PRIVATE_SENTINEL"}]),
        json!([{"type":"result","subtype":"error_max_turns","result":"PRIVATE_SENTINEL"}]),
        json!([{"type":"result","is_error":"false","result":"PRIVATE_SENTINEL"}]),
    ];
    for value in invalid {
        let error = normalize_claude_headless_output(&value.to_string(), "json").unwrap_err();
        assert!(!error.contains("PRIVATE_SENTINEL"));
    }
}

#[test]
fn claude_single_error_result_is_not_successful_answer_text() {
    let value = json!({"type":"result","is_error":true,"result":"PRIVATE_SENTINEL"});
    let error = normalize_claude_headless_output(&value.to_string(), "json").unwrap_err();
    assert!(!error.contains("PRIVATE_SENTINEL"));
}

#[test]
fn claude_single_terminal_result_requires_text_and_preserves_empty_answers() {
    for value in [
        json!({"type":"result"}),
        json!({"type":"result","result":42}),
    ] {
        assert!(normalize_claude_headless_output(&value.to_string(), "json").is_err());
    }
    let value = json!({"type":"result","result":"","session_id":"fixture-session"});
    let output = normalize_claude_headless_output(&value.to_string(), "json").unwrap();
    assert_eq!(output["response"], "");
    assert_eq!(output["session_id"], "fixture-session");
}
