//! Codex status parsing and host-context replay regressions. No provider runs.
use super::*;
use crate::manager::{provider_status_from_event, ProviderStatusEventPolicy};
use crate::providers::chat_transcript::normalize_chat_lines;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

#[test]
fn retained_inbox_output_is_status_neutral_only_with_exact_host_structure() {
    let p = make_provider();
    let retained: serde_json::Value = serde_json::from_str(include_str!(
        "../../fixtures/codex-0.153.4-inbox-output.json"
    ))
    .unwrap();
    assert!(matches!(
        p.parse_output(&retained.to_string()),
        Some(AgentEvent::Unknown)
    ));
    let mut with_null = retained.clone();
    with_null["payload"]["call_id"] = serde_json::Value::Null;
    assert!(matches!(
        p.parse_output(&with_null.to_string()),
        Some(AgentEvent::Unknown)
    ));
    for (key, value) in [
        ("call_id", "actual-model-call"),
        ("namespace", "foreign"),
        ("name", "wardian_task_delivery"),
    ] {
        let mut candidate = retained.clone();
        candidate["payload"][key] = serde_json::json!(value);
        assert!(
            matches!(
                p.parse_output(&candidate.to_string()),
                Some(AgentEvent::Generating)
            ),
            "{key}"
        );
    }
    let quoted = serde_json::json!({"type":"response_item","payload":{
        "type":"function_call_output","call_id":"call","output":retained.to_string()
    }});
    assert!(matches!(
        p.parse_output(&quoted.to_string()),
        Some(AgentEvent::Generating)
    ));
}

#[test]
fn parse_output_thread_started_event() {
    let p = make_provider();
    let line = r#"{"type":"thread.started","thread_id":"abc-123"}"#;
    let event = p.parse_output(line).unwrap();
    assert_eq!(
        event,
        AgentEvent::Init {
            session_id: "abc-123".into(),
            timestamp: None,
        }
    );
}

#[test]
fn parse_output_turn_started_event() {
    let p = make_provider();
    let line = r#"{"type":"turn.started"}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::UserQuery);
}

#[test]
fn parse_output_turn_completed_event() {
    let p = make_provider();
    let line = r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::TurnCompleted);
}

#[test]
fn parse_output_agent_message_event() {
    let p = make_provider();
    let line = r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
}

#[test]
fn parse_output_task_started_event() {
    let p = make_provider();
    let line = r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"abc"}}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
}

#[test]
fn parse_output_task_complete_event() {
    let p = make_provider();
    let line = r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"abc"}}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::TurnCompleted);
}

#[test]
fn parse_output_interrupted_turn_event() {
    let p = make_provider();
    let line = r#"{"type":"turn.aborted"}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::TurnInterrupted);
}

#[test]
fn parse_output_agent_message_does_not_change_status() {
    let p = make_provider();
    let line = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"Waiting for approval"}}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Unknown);
}

#[test]
fn parse_output_exec_command_begin_sets_generating() {
    let p = make_provider();
    let line =
        r#"{"type":"event_msg","payload":{"type":"exec_command_begin","command":"git status"}}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
}

#[test]
fn parse_output_function_call_output_resumes_processing() {
    let p = make_provider();
    let line =
        r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"abc"}}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
}

#[test]
fn parse_output_live_activity_response_items_set_generating() {
    let p = make_provider();
    for payload_type in [
        "reasoning",
        "function_call",
        "custom_tool_call",
        "custom_tool_call_output",
    ] {
        let line = format!(
            r#"{{"type":"response_item","payload":{{"type":"{}","call_id":"abc"}}}}"#,
            payload_type
        );

        assert_eq!(p.parse_output(&line).unwrap(), AgentEvent::Generating);
    }
}

#[test]
fn parse_output_response_item_function_call_requires_approval() {
    let p = make_provider();
    let line = r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\"command\":\"Get-Content foo\",\"sandbox_permissions\":\"require_escalated\",\"justification\":\"Allow reading foo?\"}"}}"#;
    assert_eq!(
        p.parse_output(line).unwrap(),
        AgentEvent::ActionRequired {
            message: "Allow reading foo?".into(),
        }
    );
}

#[test]
fn parse_output_response_item_function_call_without_approval_sets_generating() {
    let p = make_provider();
    let line = r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\"command\":\"Get-Content foo\"}"}}"#;
    assert_eq!(p.parse_output(line).unwrap(), AgentEvent::Generating);
}
const STARTUP: &str = include_str!("fixtures/startup-host-context.jsonl");

fn replay(path: &Path) -> Vec<AgentEvent> {
    let provider = make_provider();
    BufReader::new(std::fs::File::open(path).unwrap())
        .lines()
        .map(|line| {
            provider
                .parse_output(&line.unwrap())
                .expect("fixture event")
        })
        .collect()
}

#[test]
fn startup_host_context_replay_does_not_start_work() {
    let temp = tempfile::tempdir().unwrap();
    let rollout = temp.path().join("startup.jsonl");
    // Records exist before the reader opens, as on a fresh launch's first scan.
    std::fs::write(&rollout, STARTUP).unwrap();
    let events = replay(&rollout);
    assert_eq!(events.len(), 6);
    assert!(
        events
            .iter()
            .all(|event| matches!(event, AgentEvent::Unknown)),
        "{events:?}"
    );
    for status in ["Idle", "Processing...", "Action Needed"] {
        for event in &events {
            assert_eq!(
                provider_status_from_event(
                    status,
                    event,
                    ProviderStatusEventPolicy::PreserveActionRequired
                ),
                None
            );
        }
    }
    let chat = normalize_chat_lines("fixture-agent", "codex", STARTUP.lines());
    let context = chat
        .iter()
        .find(|event| event.metadata["input_origin"] == "context_injection")
        .expect("the same native record remains visible as context");
    assert_eq!(context.metadata["input_purpose"], "context");
    assert!(!chat
        .iter()
        .any(|event| event.metadata["input_origin"] == "human_input"));
}

#[test]
fn canonical_user_after_startup_replay_starts_exactly_one_query() {
    let temp = tempfile::tempdir().unwrap();
    let rollout = temp.path().join("startup.jsonl");
    std::fs::write(&rollout, STARTUP).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&rollout)
        .unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"event_msg","payload":{
        "type":"user_message","message":"Inspect the fixture."}})
    )
    .unwrap();
    drop(file);
    let events = replay(&rollout);
    assert_eq!(events.len(), 7);
    // apply_agent_event_with_policy counts/announces UserQuery for this policy.
    // Assert its actual parser inputs, without duplicating its counter logic.
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::UserQuery))
            .count(),
        1
    );
    let mut status = "Idle";
    for event in &events {
        if let Some(next) = provider_status_from_event(
            status,
            event,
            ProviderStatusEventPolicy::PreserveActionRequired,
        ) {
            status = next;
        }
    }
    assert_eq!(status, "Processing...");
}

#[test]
fn explicit_user_kinds_and_legacy_user_records_still_start_work() {
    let provider = make_provider();
    let mut records = vec![
        json!({"type":"event_msg","payload":{"type":"user_message","message":"Inspect the fixture."}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":"<environment_context>quoted by a user</environment_context>"}}),
    ];
    for kinds in [
        json!(["user.text"]),
        json!(["user.image"]),
        json!(["user.text", "user.image"]),
        json!([" user.text "]),
    ] {
        records.push(json!({"type":"response_item","payload":{
            "type":"message","role":"user","content":[{"type":"input_text",
                "text":"# AGENTS.md instructions quoted by a user"}],
            "internal_chat_message_metadata_passthrough":{"content_item_kinds":kinds}}}));
    }
    for record in records {
        let event = provider.parse_output(&record.to_string()).unwrap();
        assert_eq!(event, AgentEvent::UserQuery, "{record}");
        assert_eq!(
            provider_status_from_event(
                "Idle",
                &event,
                ProviderStatusEventPolicy::PreserveActionRequired
            ),
            Some("Processing...")
        );
        assert_eq!(
            provider_status_from_event(
                "Action Needed",
                &event,
                ProviderStatusEventPolicy::PreserveActionRequired
            ),
            None
        );
    }
}

#[test]
fn unproven_or_mixed_context_kinds_stay_neutral_without_text_matching() {
    let provider = make_provider();
    for kinds in [
        Value::Null,
        json!([]),
        json!("user.text"),
        json!([null]),
        json!(["user.future"]),
        json!(["user.text", "agents_md.instructions"]),
        json!(["agents_md.instructions", "environments.environment_context"]),
    ] {
        let record = json!({"type":"response_item","payload":{
            "type":"message","role":"user","content":[{"type":"input_text","text":"Ordinary words."}],
            "internal_chat_message_metadata_passthrough":{"content_item_kinds":kinds}}});
        assert_eq!(
            provider.parse_output(&record.to_string()),
            Some(AgentEvent::Unknown),
            "{kinds}"
        );
    }
    let no_metadata = json!({"type":"response_item","payload":{
        "type":"message","role":"user","content":[{"type":"input_text","text":"Ordinary words."}]}});
    assert_eq!(
        provider.parse_output(&no_metadata.to_string()),
        Some(AgentEvent::Unknown)
    );
}

#[test]
fn actual_activity_completion_and_approval_keep_the_existing_status_policy() {
    let provider = make_provider();
    for (record, event, next) in [
        (
            json!({"type":"turn.started"}),
            AgentEvent::UserQuery,
            "Processing...",
        ),
        (
            json!({"type":"event_msg","payload":{"type":"task_started"}}),
            AgentEvent::Generating,
            "Processing...",
        ),
        (
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"fixture-call","output":"done"}}),
            AgentEvent::Generating,
            "Processing...",
        ),
        (
            json!({"type":"event_msg","payload":{"type":"exec_approval_request","command":"fixture-command"}}),
            AgentEvent::ActionRequired {
                message: "fixture-command".into(),
            },
            "Action Needed",
        ),
        (
            json!({"type":"turn.completed"}),
            AgentEvent::TurnCompleted,
            "Idle",
        ),
        (
            json!({"type":"turn.interrupted"}),
            AgentEvent::TurnInterrupted,
            "Idle",
        ),
    ] {
        let parsed = provider.parse_output(&record.to_string()).unwrap();
        assert_eq!(parsed, event);
        assert_eq!(
            provider_status_from_event(
                "Processing...",
                &parsed,
                ProviderStatusEventPolicy::PreserveActionRequired
            ),
            Some(next)
        );
    }
}
