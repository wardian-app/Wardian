use super::*;

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    super::super::run_migrations(&conn).unwrap();
    conn
}

fn admission<'a>(message: &'a str, task: bool) -> Admission<'a> {
    Admission {
        sender: "sender",
        recipient: "receiver",
        message,
        idempotency_key: None,
        task,
        generation: 1,
    }
}

#[test]
fn startup_failure_publishes_one_correlated_reply_and_removes_task_from_dispatch() {
    let conn = database();
    let task = admit(&conn, admission("work", true)).unwrap();
    let claim = claim_next_task(&conn, "receiver", 1).unwrap().unwrap();
    let failed = reply_startup_failure(&conn, &claim).unwrap();
    assert_eq!(failed.task.status, InteractionStatus::Completed);
    assert_eq!(failed.reply.status, ReplyStatus::Failed);
    let page = receive(&conn, "sender", None, None, 100).unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(
        page.messages[0].parent_interaction_id.as_deref(),
        Some(task.record.id.as_str())
    );
    assert_eq!(page.messages[0].reply_status, Some(ReplyStatus::Failed));
    assert!(page.messages[0]
        .message
        .starts_with("Wardian delivery failed"));
    assert!(claim_next_task(&conn, "receiver", 1).unwrap().is_none());
    assert!(!owns_claim(&conn, &claim).unwrap());
    assert!(reply_startup_failure(&conn, &claim).is_err());
    assert_eq!(receive(&conn, "sender", None, None, 100).unwrap(), page);
}

#[test]
fn startup_failure_rejects_stale_accepted_and_uncertain_claims() {
    for outcome in ["provider_accepted", "uncertain"] {
        let conn = database();
        let task = admit(&conn, admission("work", true)).unwrap();
        let claim = claim_next_task(&conn, "receiver", 1).unwrap().unwrap();
        finish_claim(&conn, &claim, outcome).unwrap();
        assert!(reply_startup_failure(&conn, &claim).is_err());
        assert_eq!(
            load(&conn, &task.record.id).unwrap().status,
            InteractionStatus::AwaitingReply
        );
        assert!(receive(&conn, "sender", None, None, 100)
            .unwrap()
            .messages
            .is_empty());
    }
    let conn = database();
    admit(&conn, admission("work", true)).unwrap();
    let old = claim_next_task(&conn, "receiver", 1).unwrap().unwrap();
    release_before_write(&conn, &old).unwrap();
    let current = claim_next_task(&conn, "receiver", 2).unwrap().unwrap();
    assert!(reply_startup_failure(&conn, &old).is_err());
    assert!(owns_claim(&conn, &current).unwrap());
    reply_startup_failure(&conn, &current).unwrap();
}

#[test]
fn startup_failure_rolls_back_all_publication_on_storage_failure() {
    let conn = database();
    let task = admit(&conn, admission("work", true)).unwrap();
    let claim = claim_next_task(&conn, "receiver", 1).unwrap().unwrap();
    conn.execute_batch("CREATE TRIGGER reject_failure BEFORE INSERT ON agent_message_availability WHEN NEW.recipient='sender' BEGIN SELECT RAISE(ABORT, 'publication failed'); END;").unwrap();
    assert!(reply_startup_failure(&conn, &claim).is_err());
    assert!(owns_claim(&conn, &claim).unwrap());
    assert_eq!(
        load(&conn, &task.record.id).unwrap().status,
        InteractionStatus::AwaitingReply
    );
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM structured_replies WHERE request_id=?1",
            [&task.record.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    conn.execute_batch("DROP TRIGGER reject_failure").unwrap();
    reply_startup_failure(&conn, &claim).unwrap();
}

#[test]
fn receive_byte_budget_paginates_without_claiming_or_dropping_tail() {
    let conn = database();
    let large = "a".repeat(crate::agent_messaging::MAX_MESSAGE_BYTES);
    let first = admit(&conn, admission(&large, false)).unwrap();
    let second = admit(&conn, admission(&large, false)).unwrap();
    let tail = admit(&conn, admission("tail", true)).unwrap();
    let page = receive(&conn, "receiver", None, None, 100).unwrap();
    assert_eq!(
        page.messages
            .iter()
            .map(|m| &m.interaction_id)
            .collect::<Vec<_>>(),
        [&first.record.id, &second.record.id]
    );
    assert!(page.has_more);
    let owner: String = conn
        .query_row(
            "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
            [&tail.record.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(owner, "pending");
    let next = receive(&conn, "receiver", Some(&page.next_cursor), None, 100).unwrap();
    assert_eq!(next.messages.len(), 1);
    assert_eq!(next.messages[0].interaction_id, tail.record.id);
    assert!(!next.has_more);
    assert_eq!(receive(&conn, "receiver", None, None, 100).unwrap(), page);
}

#[test]
fn receive_serialized_budget_handles_maximum_json_expansion_and_makes_progress() {
    let conn = database();
    let escaped = "\0".repeat(crate::agent_messaging::MAX_MESSAGE_BYTES);
    let first = admit(&conn, admission(&escaped, false)).unwrap();
    let second = admit(&conn, admission(&escaped, false)).unwrap();
    let page = receive(&conn, "receiver", None, None, 100).unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].interaction_id, first.record.id);
    assert_eq!(page.messages[0].message, escaped);
    assert!(page.has_more);
    let response =
        crate::agent_messaging::AgentMessagingResponse::ReceiveMessages { page: page.clone() };
    assert!(
        serde_json::to_vec(&response).unwrap().len()
            <= crate::agent_messaging::MAX_RECEIVE_SERIALIZED_BYTES
    );
    let next = receive(&conn, "receiver", Some(&page.next_cursor), None, 100).unwrap();
    assert_eq!(next.messages[0].interaction_id, second.record.id);
    assert!(!next.has_more);
}

#[test]
fn admission_keeps_literal_body_canonical_and_info_never_runnable() {
    let conn = database();
    let text = "  λ中文😀\r\nline\n\"quote\"\\path\t\0  ";
    let admitted = admit(&conn, admission(text, false)).unwrap();
    assert_eq!(
        admitted.record.trigger_policy,
        InteractionTriggerPolicy::NotifyOnly
    );
    assert!(claim_next_task(&conn, "receiver", 1).unwrap().is_none());
    assert_eq!(
        super::super::list_mailbox_messages_with_conn(&conn)
            .unwrap()
            .len(),
        0
    );
    let page = receive(&conn, "receiver", None, None, 100).unwrap();
    assert_eq!(page.messages[0].message, text);
    assert_eq!(page.messages[0].interaction_id, admitted.record.id);
    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(agent_message_availability)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(columns, ["sequence", "recipient", "interaction_id"]);
}

#[test]
fn idempotency_is_sender_operation_scoped_and_conflicts_fail() {
    let conn = database();
    let mut input = admission("body", false);
    input.idempotency_key = Some("key");
    let first = admit(&conn, input).unwrap();
    let mut input = admission("body", false);
    input.idempotency_key = Some("key");
    let again = admit(&conn, input).unwrap();
    assert!(again.duplicate);
    assert_eq!(first.record.id, again.record.id);
    for (body, target) in [("changed", "receiver"), ("body", "other")] {
        let mut input = admission(body, false);
        input.recipient = target;
        input.idempotency_key = Some("key");
        assert_eq!(
            admit(&conn, input).err().unwrap().code,
            "idempotency_conflict"
        );
    }
    let mut input = admission("body", true);
    input.idempotency_key = Some("key");
    assert!(!admit(&conn, input).unwrap().duplicate);
}

#[test]
fn receive_replay_ack_bound_and_no_legacy_backfill() {
    let conn = database();
    let initial = receive(&conn, "receiver", None, None, 1).unwrap();
    let one = admit(&conn, admission("one", false)).unwrap();
    admit(&conn, admission("two", false)).unwrap();
    let page = receive(&conn, "receiver", Some(&initial.next_cursor), None, 1).unwrap();
    assert_eq!(page.messages[0].interaction_id, one.record.id);
    assert!(page.has_more);
    assert_eq!(
        page,
        receive(&conn, "receiver", Some(&initial.next_cursor), None, 1).unwrap()
    );
    let next = receive(&conn, "receiver", None, Some(&page.ack_cursor), 1).unwrap();
    assert_eq!(next.messages[0].message, "two");
    assert_eq!(
        page,
        receive(&conn, "receiver", Some(&initial.next_cursor), None, 1).unwrap()
    );
    assert_eq!(
        receive(&conn, "foreign", Some(&page.next_cursor), None, 1)
            .unwrap_err()
            .code,
        "invalid_cursor"
    );
    assert_eq!(
        receive(
            &conn,
            "receiver",
            None,
            Some("am1_00000000000000000000000000000000"),
            1
        )
        .unwrap_err()
        .code,
        "expired_cursor"
    );
    let mut legacy = one.record;
    legacy.id = "legacy".into();
    super::super::upsert_interaction_record_with_conn(&conn, &legacy).unwrap();
    assert_eq!(
        receive(&conn, "receiver", None, None, 100)
            .unwrap()
            .messages
            .len(),
        1
    );
}

#[test]
fn receiver_and_scheduler_have_exclusive_claims_and_uncertain_never_replays() {
    let conn = database();
    let first = admit(&conn, admission("read", true)).unwrap();
    let page = receive(&conn, "receiver", None, None, 100).unwrap();
    assert_eq!(page.messages[0].interaction_id, first.record.id);
    assert!(claim_next_task(&conn, "receiver", 2).unwrap().is_none());
    let second = admit(&conn, admission("dispatch", true)).unwrap();
    let claim = claim_next_task(&conn, "receiver", 3).unwrap().unwrap();
    assert_eq!(claim.record.id, second.record.id);
    assert_eq!(
        receive(&conn, "receiver", Some(&page.next_cursor), None, 100)
            .unwrap()
            .messages
            .len(),
        0
    );
    finish_claim(&conn, &claim, "uncertain").unwrap();
    assert!(claim_next_task(&conn, "receiver", 4).unwrap().is_none());
    assert_eq!(
        finish_claim(&conn, &claim, "provider_visible")
            .unwrap_err()
            .code,
        "stale_claim"
    );
}

#[test]
fn reply_is_authorized_atomic_correlated_and_idempotent() {
    let conn = database();
    let task = admit(&conn, admission("task", true)).unwrap();
    assert_eq!(
        reply(&conn, "foreign", &task.record.id, ReplyStatus::Done, "done")
            .err()
            .unwrap()
            .code,
        "unauthorized"
    );
    conn.execute_batch("CREATE TRIGGER fail_availability BEFORE INSERT ON agent_message_availability WHEN NEW.recipient='sender' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "done"
    )
    .is_err());
    assert_eq!(
        load(&conn, &task.record.id).unwrap().status,
        InteractionStatus::AwaitingReply
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM structured_replies", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM interactions WHERE kind='reply'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER fail_availability")
        .unwrap();
    let result = reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "done",
    )
    .unwrap();
    let again = reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "done",
    )
    .unwrap();
    assert!(again.duplicate);
    assert_eq!(again.record.id, result.record.id);
    assert_eq!(
        reply(
            &conn,
            "receiver",
            &task.record.id,
            ReplyStatus::Failed,
            "other"
        )
        .err()
        .unwrap()
        .code,
        "conflicting_reply"
    );
    let received = receive(&conn, "sender", None, None, 100).unwrap();
    assert_eq!(received.messages.len(), 1);
    assert_eq!(
        received.messages[0].parent_interaction_id,
        Some(task.record.id)
    );
    assert_eq!(received.messages[0].reply_status, Some(ReplyStatus::Done));
}

#[test]
fn admission_failure_rolls_back_canonical_record_and_key() {
    let conn = database();
    conn.execute_batch("CREATE TRIGGER fail_availability BEFORE INSERT ON agent_message_availability BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(admit(&conn, admission("x", false)).is_err());
    for table in ["interactions", "agent_message_delivery"] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn structured_projection_failure_rolls_back_reply_then_retries_once() {
    let conn = database();
    let task = admit(&conn, admission("task", true)).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_projection BEFORE INSERT ON structured_replies BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "done"
    )
    .is_err());
    assert_eq!(
        load(&conn, &task.record.id).unwrap().status,
        InteractionStatus::AwaitingReply
    );
    for query in [
        "SELECT COUNT(*) FROM interactions WHERE kind='reply'",
        "SELECT COUNT(*) FROM structured_replies",
        "SELECT COUNT(*) FROM agent_message_availability WHERE recipient='sender'",
    ] {
        assert_eq!(
            conn.query_row(query, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    conn.execute_batch("DROP TRIGGER fail_projection").unwrap();
    let first = reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "done",
    )
    .unwrap();
    let duplicate = reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "done",
    )
    .unwrap();
    assert_eq!(first.record.id, duplicate.record.id);
    assert!(duplicate.duplicate);
    assert_eq!(
        receive(&conn, "sender", None, None, 100)
            .unwrap()
            .messages
            .len(),
        1
    );
}

#[test]
fn agent_deletion_removes_messaging_references_atomically_and_preserves_other_cursors() {
    let mut conn = database();
    let task = admit(&conn, admission("task", true)).unwrap();
    reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "done",
    )
    .unwrap();
    let page = receive(&conn, "receiver", None, None, 100).unwrap();
    receive(&conn, "receiver", None, Some(&page.ack_cursor), 100).unwrap();
    let sender_page = receive(&conn, "sender", None, None, 100).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_delete BEFORE DELETE ON agent_message_delivery BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(super::super::delete_agent_with_conn(&mut conn, "receiver").is_err());
    assert!(load(&conn, &task.record.id).is_ok());
    assert!(cursor_sequence(&conn, "receiver", &page.next_cursor).is_ok());
    conn.execute_batch("DROP TRIGGER fail_delete").unwrap();
    super::super::delete_agent_with_conn(&mut conn, "receiver").unwrap();
    for table in [
        "interactions",
        "structured_replies",
        "agent_message_delivery",
        "agent_message_availability",
        "agent_message_ack",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    assert_eq!(
        cursor_sequence(&conn, "receiver", &page.next_cursor)
            .unwrap_err()
            .code,
        "expired_cursor"
    );
    assert!(
        receive(&conn, "sender", Some(&sender_page.next_cursor), None, 100)
            .unwrap()
            .messages
            .is_empty()
    );
}

#[test]
fn information_and_reply_provider_claims_share_receiver_ownership_and_exact_context() {
    let conn = database();
    let literal = "  λ😀\r\n\"quote\"\\path\0  ";
    let info = admit(&conn, admission(literal, false)).unwrap();
    let task = admit(&conn, admission("work", true)).unwrap();
    assert!(claim_information(&conn, "receiver", &task.record.id, 1)
        .unwrap()
        .is_none());
    let claim = claim_information(&conn, "receiver", &info.record.id, 1)
        .unwrap()
        .unwrap();
    let context = message_context(&conn, &claim.record).unwrap();
    assert_eq!(context.body, literal);
    assert_eq!(context.sender, "sender");
    assert_eq!(context.recipient, "receiver");
    assert_eq!(context.request_id, None);
    assert_eq!(context.kind, InteractionKind::Message);
    let encoded = serde_json::to_string(&context).unwrap();
    let decoded: crate::agent_messaging::AgentMessageContext =
        serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.body.as_bytes(), literal.as_bytes());
    finish_claim(&conn, &claim, "provider_accepted").unwrap();
    assert_eq!(
        release_before_write(&conn, &claim).unwrap_err().code,
        "stale_claim"
    );
    let received = receive(&conn, "receiver", None, None, 100).unwrap();
    assert_eq!(received.messages.len(), 1);
    assert_eq!(received.messages[0].interaction_id, task.record.id);
    let reply = reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        literal,
    )
    .unwrap();
    let claim = claim_information(&conn, "sender", &reply.record.id, 1)
        .unwrap()
        .unwrap();
    let context = message_context(&conn, &claim.record).unwrap();
    assert_eq!(context.sender, "receiver");
    assert_eq!(context.recipient, "sender");
    assert_eq!(context.kind, InteractionKind::Reply);
    assert_eq!(context.parent_interaction_id, Some(task.record.id.clone()));
    assert_eq!(context.request_id, Some(task.record.id));
    assert_eq!(context.body, literal);
    assert_eq!(context.reply_status, Some(ReplyStatus::Done));
    // Definite no-write gives the receiver a chance; its claim then excludes push.
    release_before_write(&conn, &claim).unwrap();
    assert_eq!(
        receive(&conn, "sender", None, None, 100).unwrap().messages[0].interaction_id,
        reply.record.id
    );
    assert!(claim_information(&conn, "sender", &reply.record.id, 1)
        .unwrap()
        .is_none());
}

#[test]
fn startup_information_snapshot_includes_replies_excludes_tasks_and_later_arrivals() {
    let conn = database();
    let info = admit(&conn, admission("info", false)).unwrap();
    let mut reverse = admission("earlier request", true);
    reverse.sender = "receiver";
    reverse.recipient = "sender";
    let task = admit(&conn, reverse).unwrap();
    let replied = reply(
        &conn,
        "sender",
        &task.record.id,
        ReplyStatus::Done,
        "answer",
    )
    .unwrap();
    let work = admit(&conn, admission("wake", true)).unwrap();
    let highwater = information_highwater(&conn, "receiver").unwrap();
    let later = admit(&conn, admission("later", false)).unwrap();
    assert_eq!(
        pending_information(&conn, "receiver", highwater).unwrap(),
        [info.record.id.clone(), replied.record.id.clone()]
    );
    for id in [info.record.id, replied.record.id] {
        let claim = claim_information(&conn, "receiver", &id, 1)
            .unwrap()
            .unwrap();
        finish_claim(&conn, &claim, "provider_accepted").unwrap();
    }
    assert!(pending_information(&conn, "receiver", highwater)
        .unwrap()
        .is_empty());
    let claim = claim_next_task(&conn, "receiver", 1).unwrap().unwrap();
    assert_eq!(claim.record.id, work.record.id);
    let context = message_context(&conn, &claim.record).unwrap();
    assert_eq!(context.kind, InteractionKind::Task);
    assert_eq!(context.request_id, Some(work.record.id));
    assert_eq!(
        receive(&conn, "receiver", None, None, 100)
            .unwrap()
            .messages[0]
            .interaction_id,
        later.record.id
    );
}
