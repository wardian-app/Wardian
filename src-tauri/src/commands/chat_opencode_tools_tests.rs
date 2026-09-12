use super::*;

fn fixture() -> (String, OpencodeDbPart) {
    let rows: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/opencode-tool-1.18.29.json")).unwrap();
    let row = &rows[1];
    (
        row["session_id"].as_str().unwrap().into(),
        OpencodeDbPart {
            part_id: row["part_id"].as_str().unwrap().into(),
            message_id: row["message_id"].as_str().unwrap().into(),
            part_data: row["part"].to_string(),
            message_data: row["message"].to_string(),
            part_time_created: Some(1788779276867),
            message_time_created: Some(1788779276860),
        },
    )
}

fn read(session: &str, row: &OpencodeDbPart, sequence: u64) -> Vec<AgentChatEvent> {
    project(
        "agent",
        session,
        row,
        Some("fallback"),
        Path::new("opencode.db"),
        sequence,
    )
}

#[test]
fn native_fixture_projects_structured_call_and_terminal_tool_result() {
    let (session, row) = fixture();
    let events = read(&session, &row, 5);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].metadata["tool_name"], "read");
    assert!(events[0].metadata["tool_input"]["filePath"]
        .as_str()
        .unwrap()
        .contains("scratch-"));
    assert_eq!(
        events[0].metadata["request_root_id"],
        "msg_07b8d6ebc001N8cmilUUtu1kNT"
    );
    assert_eq!(events[1].role, Some(AgentChatRole::Tool));
    assert_eq!(events[1].status, Some(AgentChatStatus::Succeeded));
    assert!(events[1]
        .text
        .as_deref()
        .unwrap()
        .contains("probe-15928793f96c825b"));
    assert_eq!(events[1].sequence, Some(6));
    assert!(events
        .iter()
        .all(|event| event.metadata["input_origin"].is_null()));
    assert_eq!(events[0].metadata["source_path"], "opencode.db");
}

#[test]
fn running_part_never_archives_a_placeholder_result_or_changes_call_identity() {
    let (session, mut row) = fixture();
    let completed = read(&session, &row, 20);
    let mut part: Value = serde_json::from_str(&row.part_data).unwrap();
    part["state"]["status"] = json!("running");
    row.part_data = part.to_string();
    let running = read(&session, &row, 3);
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, completed[0].id);
    assert_eq!(running[0].metadata, completed[0].metadata);
    assert_eq!(running[0].status, None);
    part["state"]["status"] = json!("pending");
    row.part_data = part.to_string();
    assert!(read(&session, &row, 3).is_empty());
}

#[test]
fn errors_and_empty_success_remain_native_terminal_results() {
    let (session, mut row) = fixture();
    let mut part: Value = serde_json::from_str(&row.part_data).unwrap();
    part["state"]["output"] = json!("");
    row.part_data = part.to_string();
    assert_eq!(read(&session, &row, 1)[1].text.as_deref(), Some(""));
    part["state"]["status"] = json!("error");
    part["state"]["error"] = json!("Native permission denied");
    row.part_data = part.to_string();
    let events = read(&session, &row, 1);
    assert_eq!(events[1].status, Some(AgentChatStatus::Failed));
    assert_eq!(events[1].text.as_deref(), Some("Native permission denied"));
    assert_eq!(events[1].exit_code, None);
}

#[test]
fn foreign_identity_unknown_state_and_malformed_inputs_fail_closed() {
    let (session, mut row) = fixture();
    let original: Value = serde_json::from_str(&row.part_data).unwrap();
    for field in ["sessionID", "messageID", "id"] {
        let mut part = original.clone();
        part[field] = json!("foreign");
        row.part_data = part.to_string();
        assert!(read(&session, &row, 1).is_empty());
    }
    for state in ["pending", "unknown"] {
        let mut part = original.clone();
        part["state"]["status"] = json!(state);
        row.part_data = part.to_string();
        assert!(read(&session, &row, 1).is_empty());
    }
    let mut part = original;
    part["state"]["input"] = json!("not an object");
    row.part_data = part.to_string();
    assert!(read(&session, &row, 1).is_empty());
    row.part_data = "{".into();
    assert!(read(&session, &row, 1).is_empty());
}

#[test]
fn sqlite_refresh_preserves_text_identity_and_keeps_tools_in_request_order() {
    let (session, row) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("opencode.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE message(id TEXT, session_id TEXT, data TEXT, time_created INTEGER);
        CREATE TABLE part(id TEXT, message_id TEXT, session_id TEXT, data TEXT, time_created INTEGER);").unwrap();
    for (id, role, time) in [
        ("user", "user", 1_i64),
        ("answer", "assistant", 1788779278000),
    ] {
        conn.execute(
            "INSERT INTO message VALUES (?1,?2,?3,?4)",
            rusqlite::params![id, session, json!({"role":role}).to_string(), time],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES (?1,?1,?2,?3,?4)",
            rusqlite::params![
                id,
                session,
                json!({"type":"text","text":id}).to_string(),
                time
            ],
        )
        .unwrap();
    }
    let before =
        super::super::load_opencode_db_chat_events_from_db(&db, "agent", &session).unwrap();
    conn.execute(
        "INSERT INTO message VALUES (?1,?2,?3,?4)",
        rusqlite::params![
            row.message_id,
            session,
            row.message_data,
            row.message_time_created
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![
            row.part_id,
            row.message_id,
            session,
            row.part_data,
            row.part_time_created
        ],
    )
    .unwrap();
    let mut after =
        super::super::load_opencode_db_chat_events_from_db(&db, "agent", &session).unwrap();
    super::super::sort_chat_events(&mut after);
    assert_eq!(after.len(), 4);
    assert_eq!(before[0].id, after[0].id);
    assert_eq!(before[1].id, after[3].id);
    assert_eq!(after[1].kind, AgentChatEventKind::ToolCall);
    assert_eq!(after[2].kind, AgentChatEventKind::ToolResult);
    // A part bound to another DB session must never appear in this transcript.
    conn.execute(
        "UPDATE part SET session_id='foreign' WHERE id=?1",
        [&row.part_id],
    )
    .unwrap();
    assert_eq!(
        super::super::load_opencode_db_chat_events_from_db(&db, "agent", &session)
            .unwrap()
            .len(),
        2
    );
}
