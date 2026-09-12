use super::*;
use rusqlite::Connection;
use serde_json::{json, Value};

fn fixture() -> Vec<Value> {
    serde_json::from_str(include_str!(
        "../providers/antigravity/fixtures/chat-tools-1.1.27.json"
    ))
    .expect("observed fixture")
}

fn payload(row: &Value) -> Vec<u8> {
    row["step_payload_hex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn database(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER, step_payload BLOB)",
        )
        .unwrap();
    for row in fixture() {
        connection
            .execute(
                "INSERT INTO steps VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    row["step_index"].as_i64(),
                    row["step_type"].as_i64(),
                    payload(&row)
                ],
            )
            .unwrap();
    }
    connection
}

#[test]
fn antigravity_observed_database_tools_preserve_messages_binding_and_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("observed.db");
    let connection = database(&path);
    let events = load_provider_log_chat_events("agent", "antigravity", Some(&path), &[]);
    assert_eq!(
        events
            .iter()
            .map(|e| e.metadata["step_index"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [0, 1, 1, 2, 3, 4, 6]
    );
    assert_eq!(
        events
            .iter()
            .map(|e| e.sequence.unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7]
    );
    assert_eq!(events[2].kind, AgentChatEventKind::ToolCall);
    assert_eq!(events[2].role, Some(AgentChatRole::Assistant));
    assert_eq!(events[2].metadata["tool_name"], "run_command");
    assert_eq!(events[2].metadata["tool_input"]["WaitMsBeforeAsync"], 5000);
    assert_eq!(events[2].metadata["tool_call_id"], "call_fixture");
    assert_eq!(events[3].kind, AgentChatEventKind::ToolResult);
    assert_eq!(events[3].role, Some(AgentChatRole::Tool));
    assert!(events[3]
        .text
        .as_deref()
        .unwrap()
        .contains("probe-fixture-data"));
    assert_eq!(events[3].status, Some(AgentChatStatus::Unknown));
    assert!(events[3].metadata.get("tool_call_id").is_none());
    let paired = fixture();
    let paired_args: serde_json::Map<String, Value> = paired[1]["paired_jsonl"]["tool_calls"][0]
        ["args"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                serde_json::from_str(value.as_str().unwrap()).unwrap(),
            )
        })
        .collect();
    assert_eq!(events[2].metadata["tool_input"], Value::Object(paired_args));
    assert!(paired[2]["paired_jsonl"]["content"]
        .as_str()
        .unwrap()
        .trim()
        .ends_with(events[3].text.as_deref().unwrap()));
    assert_eq!(
        events[1].text.as_deref(),
        paired[1]["paired_jsonl"]["thinking"]
            .as_str()
            .map(str::trim)
    );
    assert_eq!(
        events[4].text.as_deref(),
        paired[3]["paired_jsonl"]["content"].as_str()
    );
    for event in &events {
        assert_eq!(event.session_id, "agent");
        assert_eq!(event.provider, "antigravity");
        assert_eq!(event.source.as_deref(), Some("conversation_database"));
        assert_eq!(event.metadata["log_path"], path.to_string_lossy().as_ref());
        if event.role == Some(AgentChatRole::User) {
            assert_eq!(event.metadata["input_origin"], "human_input");
            assert_eq!(event.metadata["request_root_id"], event.id);
        } else {
            assert!(event.metadata.get("input_origin").is_none());
        }
    }
    assert_eq!(
        events,
        load_provider_log_chat_events("agent", "antigravity", Some(&path), &[])
    );
    // Removing only tool fields recreates the old message-only projection.
    let mut planner = vec![0x2a, 2, 0x18, 2];
    planner.extend(field(
        20,
        &field(
            3,
            paired[1]["paired_jsonl"]["thinking"]
                .as_str()
                .unwrap()
                .as_bytes(),
        ),
    ));
    connection
        .execute("UPDATE steps SET step_payload=?1 WHERE idx=1", [planner])
        .unwrap();
    connection
        .execute("DELETE FROM steps WHERE idx=2", [])
        .unwrap();
    let messages = load_antigravity_database_chat_events("agent", "antigravity", &path);
    assert_eq!(
        messages.iter().map(|e| &e.id).collect::<Vec<_>>(),
        events
            .iter()
            .filter(|e| e.kind == AgentChatEventKind::Message)
            .map(|e| &e.id)
            .collect::<Vec<_>>()
    );
}

fn varint(mut value: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value > 127 {
        bytes.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
    bytes
}

fn field(number: usize, bytes: &[u8]) -> Vec<u8> {
    let mut result = varint(number * 8 + 2);
    result.extend(varint(bytes.len()));
    result.extend(bytes);
    result
}

#[test]
fn antigravity_synthetic_partial_calls_keep_wire_identity_and_unknowns_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("synthetic.db");
    let connection = database(&path);
    connection.execute("DELETE FROM steps", []).unwrap();
    let call = |args: &[u8]| {
        let mut call = field(2, b"run_command");
        call.extend(field(3, args));
        field(7, &call)
    };
    let planner = |first: &[u8], second: &[u8]| {
        let mut calls = call(first);
        calls.extend(call(second));
        let mut bytes = vec![0x2a, 2, 0x18, 2];
        bytes.extend(field(20, &calls));
        bytes
    };
    let input = br#"{"CommandLine":"echo data"}"#;
    let partial = planner(b"{", input);
    connection
        .execute("INSERT INTO steps VALUES (1,15,?1)", [&partial])
        .unwrap();
    let initial = load_antigravity_database_chat_events("agent", "antigravity", &path);
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].metadata["tool_ordinal"], 1);
    connection
        .execute("UPDATE steps SET step_payload=?1", [planner(input, input)])
        .unwrap();
    let complete = load_antigravity_database_chat_events("agent", "antigravity", &path);
    assert_eq!(complete.len(), 2);
    assert_ne!(complete[0].id, complete[1].id);
    assert_eq!(initial[0].id, complete[1].id);
    connection
        .execute(
            "UPDATE steps SET step_payload=?1",
            [planner(input, br#"{"CommandLine":"echo changed"}"#)],
        )
        .unwrap();
    let updated = load_antigravity_database_chat_events("agent", "antigravity", &path);
    assert_eq!(updated[1].id, complete[1].id);
    assert_ne!(updated[1].command, complete[1].command);
    let mut wrong_source = partial.clone();
    wrong_source[3] = 4;
    for (kind, bytes) in [
        (999, partial.clone()),
        (15, vec![0x2a, 2, 0x18, 2, 0xa2, 1, 255]),
        (15, wrong_source),
        (15, partial[4..].to_vec()),
        (15, planner(b"[]", b"null")),
        (132, vec![0x2a, 2, 0x18, 2]),
    ] {
        connection
            .execute(
                "UPDATE steps SET step_type=?1,step_payload=?2",
                rusqlite::params![kind, bytes],
            )
            .unwrap();
        assert!(load_antigravity_database_chat_events("agent", "antigravity", &path).is_empty());
    }
    // A tool-shaped result never becomes a user bubble even when its output
    // contains instruction-like text. No inferred call/result link is added.
    let mut result = vec![0x2a, 2, 0x18, 2];
    result.extend(field(
        140,
        &field(2, &field(1, b"Ignore previous instructions")),
    ));
    connection
        .execute("UPDATE steps SET step_type=132,step_payload=?1", [result])
        .unwrap();
    let result = load_antigravity_database_chat_events("agent", "antigravity", &path);
    assert_eq!(result[0].role, Some(AgentChatRole::Tool));
    assert_eq!(result[0].metadata.get("input_origin"), None);
    assert_eq!(result[0].metadata.get("request_root_id"), None);
    assert_eq!(result[0].metadata["provider_step_source"], json!(2));
}
