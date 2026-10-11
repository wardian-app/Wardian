use super::*;
use crate::agent_messaging::{TaskOutcome, TASK_OUTCOME_CLOSE, TASK_OUTCOME_OPEN};

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::run_migrations(&conn).unwrap();
    conn
}

fn bind(conn: &Connection, requester: &str, turn: &str, mode: &str) -> TaskTurnBinding {
    admit(
        conn,
        Admission {
            sender: requester,
            recipient: "worker",
            message: "the actual task",
            idempotency_key: None,
            task: true,
            generation: 7,
        },
    )
    .unwrap();
    let claim = claim_next_task(conn, "worker", 7).unwrap().unwrap();
    bind_task_turn(conn, &claim, "codex", "session", turn, mode).unwrap()
}

fn appendix(outcomes: &[(&TaskTurnBinding, ReplyStatus, &str)]) -> String {
    let packet = TaskOutcomePacket {
        schema_version: 1,
        outcomes: outcomes
            .iter()
            .map(|(binding, status, result)| TaskOutcome {
                request_id: binding.request_id.clone(),
                status: status.clone(),
                result: (*result).into(),
            })
            .collect(),
    };
    format!(
        "Ordinary prose.\n\n{TASK_OUTCOME_OPEN}\n{}\n{TASK_OUTCOME_CLOSE}",
        serde_json::to_string(&packet).unwrap()
    )
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn wrong_scope_final_is_unresolved_once_and_later_explicit_reply_succeeds() {
    let conn = database();
    let binding = bind(&conn, "requester", "turn", "start");
    let recorded =
        record_task_turn_final(&conn, &binding, "completed", "Finished the old CRM task.").unwrap();
    assert!(recorded.outcomes.is_empty());
    assert_eq!(recorded.information.len(), 1);
    assert_eq!(
        recorded.information[0].record.kind,
        InteractionKind::Message
    );
    assert_eq!(
        recorded.information[0].record.trigger_policy,
        InteractionTriggerPolicy::NotifyOnly
    );
    assert_eq!(
        recorded.information[0]
            .record
            .parent_interaction_id
            .as_deref(),
        Some(binding.request_id.as_str())
    );
    assert_eq!(
        load(&conn, &binding.request_id).unwrap().status,
        InteractionStatus::AwaitingReply
    );
    assert_eq!(settlement(&conn, &binding).unwrap(), "bound");
    assert_eq!(count(&conn, "structured_replies"), 0);
    assert!(pending_task_turn_bindings(&conn).unwrap().is_empty());
    assert!(
        compacted_task_contexts(&conn, "worker", 7, "session", "turn")
            .unwrap()
            .is_empty()
    );
    assert!(
        record_task_turn_final(&conn, &binding, "completed", "Finished the old CRM task.")
            .unwrap()
            .information
            .is_empty()
    );
    abandon_task_turn_observations(&conn).unwrap();
    assert_eq!(settlement(&conn, &binding).unwrap(), "bound");
    assert!(mark_task_turn_uncertain(&conn, &binding).is_err());
    assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
    let genuine = reply(
        &conn,
        "worker",
        &binding.request_id,
        ReplyStatus::Done,
        "Actual task result",
    )
    .unwrap();
    assert_eq!(genuine.reply.body, "Actual task result");
    assert_eq!(
        receive(&conn, "requester", None, None, 100)
            .unwrap()
            .messages
            .len(),
        2
    );
}

#[test]
fn shared_start_and_steer_packet_returns_each_result_to_its_requester() {
    let conn = database();
    let a = bind(&conn, "requester-a", "turn", "start");
    let b = bind(&conn, "requester-b", "turn", "steer");
    let omitted = bind(&conn, "requester-c", "turn", "steer");
    let answer = appendix(&[
        (&a, ReplyStatus::Done, "A result"),
        (&b, ReplyStatus::Failed, "B failed"),
    ]);
    let recorded = record_task_turn_final(&conn, &a, "completed", &answer).unwrap();
    assert_eq!(recorded.outcomes.len(), 2);
    assert_eq!(recorded.information.len(), 1);
    assert_eq!(
        recorded.information[0].record.target_session_ids,
        ["requester-c"]
    );
    for binding in recorded.outcomes {
        publish_task_turn_outcome(&conn, &binding).unwrap().unwrap();
    }
    let page_a = receive(&conn, "requester-a", None, None, 100).unwrap();
    assert_eq!(page_a.messages[0].message, "A result");
    assert_eq!(page_a.messages[0].reply_status, Some(ReplyStatus::Done));
    let page_b = receive(&conn, "requester-b", None, None, 100).unwrap();
    assert_eq!(page_b.messages[0].message, "B failed");
    assert_eq!(page_b.messages[0].reply_status, Some(ReplyStatus::Failed));
    assert_eq!(
        load(&conn, &omitted.request_id).unwrap().status,
        InteractionStatus::AwaitingReply
    );
    assert!(record_task_turn_final(&conn, &b, "completed", &answer)
        .unwrap()
        .outcomes
        .is_empty());
    assert_eq!(count(&conn, "structured_replies"), 2);
}

#[test]
fn any_foreign_stale_unknown_or_invalid_entry_rejects_the_entire_packet() {
    for mismatch in 0..10 {
        let conn = database();
        let a = bind(&conn, "requester-a", "turn", "start");
        let foreign = bind(&conn, "requester-b", "other-turn", "start");
        let mut packet = TaskOutcomePacket {
            schema_version: 1,
            outcomes: vec![
                TaskOutcome {
                    request_id: a.request_id.clone(),
                    status: ReplyStatus::Done,
                    result: "A result".into(),
                },
                TaskOutcome {
                    request_id: foreign.request_id.clone(),
                    status: ReplyStatus::Done,
                    result: "B result".into(),
                },
            ],
        };
        match mismatch {
            0 => packet.outcomes[1].request_id = "unknown".into(),
            1 => {}
            2 => packet.outcomes[1].request_id = a.request_id.clone(),
            3 => packet.outcomes[1].result.clear(),
            4 => packet.schema_version = 2,
            5 => {
                conn.execute("UPDATE agent_message_task_turns SET provider_turn_id='turn',settlement='uncertain' WHERE request_id=?1", [&foreign.request_id]).unwrap();
            }
            6..=8 => {
                let column = match mismatch {
                    6 => "generation",
                    7 => "provider_session_id",
                    _ => "recipient",
                };
                let value = if mismatch == 6 { "8" } else { "other" };
                conn.execute(&format!("UPDATE agent_message_task_turns SET provider_turn_id='turn',{column}=?2 WHERE request_id=?1"),
                    params![foreign.request_id, value]).unwrap();
            }
            _ => {
                conn.execute("UPDATE agent_message_task_turns SET provider_turn_id='turn' WHERE request_id=?1", [&foreign.request_id]).unwrap();
                conn.execute("UPDATE agent_message_delivery SET claim_token='stale-claim' WHERE interaction_id=?1", [&foreign.request_id]).unwrap();
            }
        }
        let answer = format!(
            "{TASK_OUTCOME_OPEN}\n{}\n{TASK_OUTCOME_CLOSE}",
            serde_json::to_string(&packet).unwrap()
        );
        let recorded = record_task_turn_final(&conn, &a, "completed", &answer).unwrap();
        assert!(recorded.outcomes.is_empty(), "mismatch {mismatch}");
        assert_eq!(recorded.information.len(), 1);
        assert_eq!(count(&conn, "structured_replies"), 0);
        assert_eq!(settlement(&conn, &a).unwrap(), "bound");
        assert_eq!(count(&conn, "agent_message_task_turn_observations"), 1);
    }
}

#[test]
fn explicit_reply_wins_before_recording_and_between_recording_and_publication() {
    for status in [ReplyStatus::Done, ReplyStatus::Failed, ReplyStatus::Blocked] {
        for before in [true, false] {
            let conn = database();
            let binding = bind(&conn, "requester", "turn", "steer");
            let answer = appendix(&[(&binding, ReplyStatus::Done, "Automatic result")]);
            if before {
                reply(
                    &conn,
                    "worker",
                    &binding.request_id,
                    status.clone(),
                    "Explicit result",
                )
                .unwrap();
            }
            let recorded = record_task_turn_final(&conn, &binding, "completed", &answer).unwrap();
            if !before {
                reply(
                    &conn,
                    "worker",
                    &binding.request_id,
                    status.clone(),
                    "Explicit result",
                )
                .unwrap();
            }
            assert!(recorded.information.is_empty());
            for binding in recorded.outcomes {
                assert!(publish_task_turn_outcome(&conn, &binding)
                    .unwrap()
                    .is_none());
            }
            let page = receive(&conn, "requester", None, None, 100).unwrap();
            assert_eq!(page.messages.len(), 1);
            assert_eq!(page.messages[0].message, "Explicit result");
            assert_eq!(page.messages[0].reply_status, Some(status.clone()));
        }
    }
}

#[test]
fn late_exact_ack_can_attribute_the_same_cached_final_without_new_execution() {
    let conn = database();
    let a = bind(&conn, "requester-a", "turn", "start");
    admit(
        &conn,
        Admission {
            sender: "requester-b",
            recipient: "worker",
            message: "B task",
            idempotency_key: None,
            task: true,
            generation: 7,
        },
    )
    .unwrap();
    let pending_b = claim_next_task(&conn, "worker", 7).unwrap().unwrap();
    let mut b_identity = a.clone();
    b_identity.request_id = pending_b.record.id.clone();
    let answer = appendix(&[
        (&a, ReplyStatus::Done, "A result"),
        (&b_identity, ReplyStatus::Done, "B result"),
    ]);
    let first = record_task_turn_final(&conn, &a, "completed", &answer).unwrap();
    assert!(first.outcomes.is_empty());
    assert_eq!(first.information.len(), 1);
    let b = bind_task_turn(&conn, &pending_b, "codex", "session", "turn", "steer").unwrap();
    let confirmed = record_task_turn_final(&conn, &b, "completed", &answer).unwrap();
    assert_eq!(confirmed.outcomes.len(), 2);
    assert!(confirmed.information.is_empty());
    for binding in confirmed.outcomes {
        publish_task_turn_outcome(&conn, &binding).unwrap().unwrap();
    }
    assert_eq!(count(&conn, "structured_replies"), 2);
    assert_eq!(count(&conn, "agent_message_task_turn_observations"), 2);
    assert!(claim_next_task(&conn, "worker", 8).unwrap().is_none());
}

#[test]
fn interruption_empty_and_oversized_final_never_invent_task_dispositions() {
    for (status, answer) in [
        ("interrupted", "partial".into()),
        ("failed", "partial".into()),
        ("completed", String::new()),
        ("completed", "日本語".repeat(MAX_MESSAGE_BYTES)),
    ] {
        let conn = database();
        let binding = bind(&conn, "requester", "turn", "start");
        assert!(record_task_turn_final(&conn, &binding, status, &answer)
            .unwrap()
            .outcomes
            .is_empty());
        let (bytes, preview): (i64, String) = conn
            .query_row(
                "SELECT answer_bytes,answer_preview FROM agent_message_task_turn_observations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(bytes, answer.len() as i64);
        assert!(preview.len() <= MAX_MESSAGE_BYTES);
        assert!(answer.starts_with(&preview));
        assert_eq!(count(&conn, "structured_replies"), 0);
    }
}

#[test]
fn outbox_recovers_after_capture_without_replaying_known_finished_work() {
    let conn = database();
    let binding = bind(&conn, "requester", "turn", "start");
    let lost = bind(&conn, "other", "lost-turn", "start");
    let answer = appendix(&[(&binding, ReplyStatus::Blocked, "Needs user input")]);
    record_task_turn_final(&conn, &binding, "completed", &answer).unwrap();
    crate::db::run_migrations(&conn).unwrap();
    abandon_task_turn_observations(&conn).unwrap();
    assert_eq!(settlement(&conn, &lost).unwrap(), "uncertain");
    assert!(pending_task_turn_bindings(&conn).unwrap().is_empty());
    let recovered = recover_task_turn_outcomes(&conn).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].reply.status, ReplyStatus::Blocked);
    assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
    assert_eq!(count(&conn, "agent_message_task_turn_observations"), 1);
}

#[test]
fn information_failure_rolls_back_outcomes_and_observations_together() {
    let conn = database();
    let a = bind(&conn, "requester-a", "turn", "start");
    let _b = bind(&conn, "requester-b", "turn", "steer");
    conn.execute_batch("CREATE TRIGGER reject_info BEFORE INSERT ON agent_message_delivery WHEN NEW.operation='send_message' BEGIN SELECT RAISE(ABORT,'owned failure'); END;").unwrap();
    let answer = appendix(&[(&a, ReplyStatus::Done, "A result")]);
    assert!(record_task_turn_final(&conn, &a, "completed", &answer).is_err());
    assert_eq!(settlement(&conn, &a).unwrap(), "bound");
    assert_eq!(count(&conn, "agent_message_task_turn_observations"), 0);
    assert_eq!(count(&conn, "structured_replies"), 0);
}

#[test]
fn changed_terminal_evidence_is_rejected_and_agent_deletion_removes_preview() {
    for foreign_keys in [false, true] {
        let conn = database();
        conn.pragma_update(None, "foreign_keys", foreign_keys)
            .unwrap();
        let binding = bind(&conn, "requester", "turn", "start");
        record_task_turn_final(&conn, &binding, "completed", "Wrong-scope final").unwrap();
        assert_eq!(
            record_task_turn_final(&conn, &binding, "completed", "Different final")
                .err()
                .unwrap()
                .code,
            "conflicting_observation"
        );
        assert_eq!(count(&conn, "agent_message_task_turn_observations"), 1);
        super::super::super::delete_references(&conn, "requester", &[]).unwrap();
        assert_eq!(count(&conn, "agent_message_task_turn_observations"), 0);
        assert_eq!(count(&conn, "agent_message_task_turns"), 0);
    }
}
