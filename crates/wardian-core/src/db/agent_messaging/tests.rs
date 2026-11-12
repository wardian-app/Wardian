use super::*;

fn has_unacknowledged_messages(conn: &Connection, recipient: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM agent_message_availability a
            LEFT JOIN agent_message_delivery d ON d.interaction_id=a.interaction_id
            WHERE a.recipient=?1
            AND a.sequence > COALESCE(
                (SELECT sequence FROM agent_message_ack WHERE recipient=?1),0
            )
            AND (d.owner IS NULL OR d.owner IN ('stored','pending','receiver_available'))
        )",
        [recipient],
        |row| row.get(0),
    )?)
}

fn reply_continuation_state(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT continuation_state FROM agent_message_delivery
             WHERE interaction_id=?1 AND operation='reply'",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    super::super::run_migrations(&conn).unwrap();
    conn
}

#[test]
fn host_information_is_exact_idempotent_and_uses_existing_claims() {
    let conn = database();
    let body = "\u{feff}café 日本語\r\n' \" `$() & | ; {{run.id}}\r\nVerdict: revise\r\n";
    let first =
        admit_host_automation_message(&conn, "run", "deliver", "receiver", body, 7).unwrap();
    assert!(!first.duplicate);
    assert_eq!(first.delivery_state, "stored");
    assert_eq!(first.record.kind, InteractionKind::Message);
    assert_eq!(
        first.record.trigger_policy,
        InteractionTriggerPolicy::NotifyOnly
    );
    assert!(first.record.sender_session_id.is_none());
    assert!(claim_next_task(&conn, "receiver", 7).unwrap().is_none());
    let replay =
        admit_host_automation_message(&conn, "run", "deliver", "receiver", body, 8).unwrap();
    assert!(replay.duplicate);
    assert_eq!(first.record.id, replay.record.id);
    let claim = claim_information(&conn, "receiver", &first.record.id, 8)
        .unwrap()
        .unwrap();
    assert!(claim_information(&conn, "receiver", &first.record.id, 8)
        .unwrap()
        .is_none());
    let replay =
        admit_host_automation_message(&conn, "run", "deliver", "receiver", body, 8).unwrap();
    assert_eq!(replay.delivery_state, "dispatching");
    release_before_write(&conn, &claim).unwrap();
    let page = receive(&conn, "receiver", None, None, 100).unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].message.as_bytes(), body.as_bytes());
    assert_eq!(
        page.messages[0].host_automation.as_ref().unwrap().run_id,
        "run"
    );
    assert_eq!(
        page.messages[0].host_automation.as_ref().unwrap().node,
        "deliver"
    );
    assert!(receive(&conn, "wrong-recipient", None, None, 100)
        .unwrap()
        .messages
        .is_empty());
    for (recipient, message) in [("other", body), ("receiver", "changed")] {
        assert_eq!(
            admit_host_automation_message(&conn, "run", "deliver", recipient, message, 8)
                .err()
                .unwrap()
                .code,
            "idempotency_conflict"
        );
    }
}

#[test]
fn host_information_storage_failure_is_atomic_and_uncertain_receipt_reconciles() {
    let conn = database();
    conn.execute_batch("CREATE TRIGGER reject_message BEFORE INSERT ON agent_message_availability BEGIN SELECT RAISE(ABORT, 'storage failure'); END;").unwrap();
    assert!(admit_host_automation_message(&conn, "run", "node", "receiver", "body", 0).is_err());
    assert!(
        reconcile_host_automation_message(&conn, "run", "node", None)
            .unwrap()
            .is_none()
    );
    for table in [
        "interactions",
        "agent_message_delivery",
        "agent_message_host_tasks",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
    conn.execute_batch("DROP TRIGGER reject_message").unwrap();
    let committed =
        admit_host_automation_message(&conn, "run", "node", "receiver", "body", 0).unwrap();
    let recovered =
        reconcile_host_automation_message(&conn, "run", "node", Some(("receiver", "body")))
            .unwrap()
            .unwrap();
    assert_eq!(committed.record.id, recovered.record.id);
    assert!(recovered.duplicate);
    conn.execute("DELETE FROM agent_message_host_tasks", [])
        .unwrap();
    assert!(reconcile_host_automation_message(&conn, "run", "node", None).is_err());
    assert!(admit_host_automation_message(&conn, "run", "node", "receiver", "body", 0).is_err());
    conn.execute("DROP TABLE agent_message_delivery", [])
        .unwrap();
    assert!(reconcile_host_automation_message(&conn, "run", "node", None).is_err());
}

#[test]
fn concurrent_host_information_replays_admit_once() {
    let conn = std::sync::Arc::new(std::sync::Mutex::new(database()));
    let gate = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let conn = conn.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                // Production admission uses the same serialized database boundary.
                let conn = conn.lock().unwrap();
                admit_host_automation_message(&conn, "run", "node", "receiver", "body", 0).unwrap()
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| !result.duplicate).count(), 1);
    assert!(results
        .iter()
        .all(|result| result.record.id == results[0].record.id));
    assert_eq!(
        receive(&conn.lock().unwrap(), "receiver", None, None, 100)
            .unwrap()
            .messages
            .len(),
        1
    );
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
fn mailbox_activity_query_is_read_only_and_tracks_acknowledged_frontier() {
    let conn = database();
    assert!(!has_unacknowledged_messages(&conn, "receiver").unwrap());
    let admitted = admit(&conn, admission("information", false)).unwrap();
    let owner_before: String = conn
        .query_row(
            "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
            [&admitted.record.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(has_unacknowledged_messages(&conn, "receiver").unwrap());
    let owner_after: String = conn
        .query_row(
            "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
            [&admitted.record.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        owner_after, owner_before,
        "wait checks must not claim messages"
    );
    let acknowledgements: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_message_ack WHERE recipient='receiver'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(acknowledgements, 0, "wait checks must not advance cursors");

    let page = receive(&conn, "receiver", None, None, 100).unwrap();
    assert_eq!(page.messages.len(), 1);
    assert!(has_unacknowledged_messages(&conn, "receiver").unwrap());
    receive(&conn, "receiver", None, Some(&page.ack_cursor), 100).unwrap();
    assert!(!has_unacknowledged_messages(&conn, "receiver").unwrap());
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
fn migration_keeps_old_stored_fyi_non_waking_and_new_admission_wakeable() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE agent_message_delivery (
            interaction_id TEXT PRIMARY KEY, sender TEXT NOT NULL, recipient TEXT NOT NULL,
            operation TEXT NOT NULL, idempotency_key TEXT, fingerprint TEXT NOT NULL,
            owner TEXT NOT NULL, generation INTEGER NOT NULL, claim_token TEXT,
            UNIQUE(sender, operation, idempotency_key));
         INSERT INTO agent_message_delivery
            (interaction_id,sender,recipient,operation,fingerprint,owner,generation)
            VALUES('old-fyi','sender','receiver','send_message','fingerprint','stored',2);",
    )
    .unwrap();
    super::super::run_migrations(&conn).unwrap();
    conn.execute(
        "INSERT INTO agent_message_availability(recipient,interaction_id)
         VALUES('receiver','old-fyi')",
        [],
    )
    .unwrap();

    assert!(pending_wakeable_information(&conn, "receiver")
        .unwrap()
        .is_empty());
    assert_eq!(recipient_origin(&conn, "old-fyi").unwrap(), None);

    let origin = RecipientOrigin {
        provider: "antigravity".into(),
        conversation_id: "conversation-current".into(),
        generation: 4,
    };
    let new =
        admit_with_wake_on_admission(&conn, admission("new FYI", false), Some(&origin)).unwrap();
    assert_eq!(
        pending_wakeable_information(&conn, "receiver").unwrap(),
        vec![new.record.id.clone()]
    );
    assert_eq!(
        recipient_origin(&conn, &new.record.id).unwrap(),
        Some(origin)
    );
}

#[test]
fn duplicate_legacy_message_cannot_be_promoted_into_a_wake() {
    let conn = database();
    let mut legacy = admission("same body", false);
    legacy.idempotency_key = Some("same-key");
    let old = admit(&conn, legacy).unwrap();
    let origin = RecipientOrigin {
        provider: "codex".into(),
        conversation_id: "thread".into(),
        generation: 8,
    };
    let mut replay = admission("same body", false);
    replay.idempotency_key = Some("same-key");
    let duplicate = admit_with_wake_on_admission(&conn, replay, Some(&origin)).unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(old.record.id, duplicate.record.id);
    assert!(pending_wakeable_information(&conn, "receiver")
        .unwrap()
        .is_empty());
    assert_eq!(recipient_origin(&conn, &old.record.id).unwrap(), None);
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
fn reply_delivery_inherits_requester_origin_from_followup_task() {
    let conn = database();
    let task = admit(&conn, admission("task", true)).unwrap();
    conn.execute(
        "UPDATE agent_message_delivery
         SET requester_provider='antigravity',
             requester_conversation_id='conversation-original',
             requester_generation=17
         WHERE interaction_id=?1",
        [&task.record.id],
    )
    .unwrap();

    let reply = reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "result",
    )
    .unwrap();
    let origin: (Option<String>, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT requester_provider, requester_conversation_id, requester_generation
             FROM agent_message_delivery WHERE interaction_id=?1",
            [&reply.record.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(
        origin,
        (
            Some("antigravity".into()),
            Some("conversation-original".into()),
            Some(17),
        )
    );
    assert_eq!(
        reply.record.parent_interaction_id.as_deref(),
        Some(task.record.id.as_str())
    );
}

#[test]
fn followup_admission_persists_requester_origin_and_replay_keeps_first_binding() {
    let conn = database();
    let first_origin = RequesterOrigin {
        provider: "antigravity".into(),
        conversation_id: "conversation-1".into(),
        generation: 12,
    };
    let mut request = admission("task", true);
    request.idempotency_key = Some("same-request");
    let first = admit_with_requester_origin(&conn, request, Some(&first_origin)).unwrap();
    assert_eq!(
        requester_origin(&conn, &first.record.id).unwrap(),
        Some(first_origin.clone())
    );

    let replacement_origin = RequesterOrigin {
        provider: "antigravity".into(),
        conversation_id: "conversation-2".into(),
        generation: 13,
    };
    let mut replay = admission("task", true);
    replay.idempotency_key = Some("same-request");
    let duplicate = admit_with_requester_origin(&conn, replay, Some(&replacement_origin)).unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.record.id, first.record.id);
    assert_eq!(
        requester_origin(&conn, &first.record.id).unwrap(),
        Some(first_origin)
    );
}

#[test]
fn unbound_reply_is_durably_blocked_without_automatic_claim() {
    let conn = database();
    let task = admit(&conn, admission("legacy task", true)).unwrap();
    let reply = reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "result",
    )
    .unwrap();

    assert_eq!(
        reply_continuation_state(&conn, &reply.record.id)
            .unwrap()
            .as_deref(),
        Some("blocked_legacy_uncertain")
    );
    assert_eq!(
        pending_reply_continuations(&conn, "sender").unwrap(),
        Vec::<String>::new()
    );
    assert!(
        claim_reply_continuation(&conn, "sender", &reply.record.id, 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn migration_marks_preexisting_unbound_replies_legacy_uncertain() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE agent_message_delivery (
            interaction_id TEXT PRIMARY KEY, sender TEXT NOT NULL, recipient TEXT NOT NULL,
            operation TEXT NOT NULL, idempotency_key TEXT, fingerprint TEXT NOT NULL,
            owner TEXT NOT NULL, generation INTEGER NOT NULL, claim_token TEXT,
            UNIQUE(sender, operation, idempotency_key));
         INSERT INTO agent_message_delivery
            (interaction_id,sender,recipient,operation,fingerprint,owner,generation)
            VALUES('old-reply','worker','requester','reply','','stored',0);",
    )
    .unwrap();

    migrate(&conn).unwrap();

    assert_eq!(
        reply_continuation_state(&conn, "old-reply")
            .unwrap()
            .as_deref(),
        Some("blocked_legacy_uncertain")
    );
    assert_eq!(requester_origin(&conn, "old-reply").unwrap(), None);
}

#[test]
fn migration_blocks_partial_pending_reply_origins_without_rewriting_terminal_states() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE agent_message_delivery (
            interaction_id TEXT PRIMARY KEY, sender TEXT NOT NULL, recipient TEXT NOT NULL,
            operation TEXT NOT NULL, idempotency_key TEXT, fingerprint TEXT NOT NULL,
            owner TEXT NOT NULL, generation INTEGER NOT NULL, claim_token TEXT,
            requester_provider TEXT, requester_conversation_id TEXT,
            requester_generation INTEGER, continuation_state TEXT,
            UNIQUE(sender, operation, idempotency_key));
         INSERT INTO agent_message_delivery
            (interaction_id,sender,recipient,operation,fingerprint,owner,generation,
             requester_provider,requester_conversation_id,requester_generation,continuation_state)
            VALUES
            ('partial','worker','requester','reply','','stored',0,'codex',NULL,3,'pending'),
            ('delivered','worker','requester','reply','','delivered',3,NULL,NULL,NULL,'delivered');",
    )
    .unwrap();

    migrate(&conn).unwrap();

    assert_eq!(
        reply_continuation_state(&conn, "partial")
            .unwrap()
            .as_deref(),
        Some("blocked_legacy_uncertain")
    );
    assert_eq!(
        reply_continuation_state(&conn, "delivered")
            .unwrap()
            .as_deref(),
        Some("delivered")
    );
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
    let requester = RequesterOrigin {
        provider: "codex".into(),
        conversation_id: "sender-thread".into(),
        generation: 1,
    };
    let task =
        admit_with_requester_origin(&conn, admission("work", true), Some(&requester)).unwrap();
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
    let claim = claim_reply_continuation(&conn, "sender", &reply.record.id, 1)
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
    assert!(
        claim_reply_continuation(&conn, "sender", &reply.record.id, 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn delivered_or_uncertain_reply_continuations_cannot_be_claimed_or_reopened() {
    let conn = database();
    let requester = RequesterOrigin {
        provider: "antigravity".into(),
        conversation_id: "conversation-original".into(),
        generation: 17,
    };
    let task =
        admit_with_requester_origin(&conn, admission("work", true), Some(&requester)).unwrap();
    let reply = reply(
        &conn,
        "receiver",
        &task.record.id,
        ReplyStatus::Done,
        "result",
    )
    .unwrap();
    let claim = claim_reply_continuation(&conn, "sender", &reply.record.id, 18)
        .unwrap()
        .unwrap();
    finish_claim(&conn, &claim, "provider_visible").unwrap();

    assert!(!reply_continuation_is_pending(&conn, "sender", &reply.record.id).unwrap());
    assert!(
        claim_reply_continuation(&conn, "sender", &reply.record.id, 19)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reply_continuation_state(&conn, &reply.record.id)
            .unwrap()
            .as_deref(),
        Some("delivered")
    );
}

#[test]
fn startup_information_and_reply_snapshots_are_separate_and_bounded() {
    let conn = database();
    let info = admit(&conn, admission("info", false)).unwrap();
    let mut reverse = admission("earlier request", true);
    reverse.sender = "receiver";
    reverse.recipient = "sender";
    let requester = RequesterOrigin {
        provider: "antigravity".into(),
        conversation_id: "receiver-conversation".into(),
        generation: 4,
    };
    let task = admit_with_requester_origin(&conn, reverse, Some(&requester)).unwrap();
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
        pending_information(&conn, "receiver", highwater)
            .unwrap()
            .as_slice(),
        std::slice::from_ref(&info.record.id)
    );
    assert_eq!(
        pending_reply_continuations(&conn, "receiver")
            .unwrap()
            .as_slice(),
        std::slice::from_ref(&replied.record.id)
    );
    let claim = claim_information(&conn, "receiver", &info.record.id, 1)
        .unwrap()
        .unwrap();
    finish_claim(&conn, &claim, "provider_accepted").unwrap();
    let reply_claim = claim_reply_continuation(&conn, "receiver", &replied.record.id, 9)
        .unwrap()
        .unwrap();
    finish_claim(&conn, &reply_claim, "provider_accepted").unwrap();
    assert!(pending_information(&conn, "receiver", highwater)
        .unwrap()
        .is_empty());
    assert!(pending_reply_continuations(&conn, "receiver")
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

// These tests use the real file resolver to obtain proofs and an isolated
// in-memory canonical DB. No global connection or synthetic proof is needed.
fn claude_mod_claim(conn: &Connection, operation: &str) -> TaskClaim {
    match operation {
        "followup_task" => {
            admit(conn, admission("work", true)).unwrap();
            claim_next_task(conn, "receiver", 43).unwrap().unwrap()
        }
        "send_message" => {
            let message = admit(conn, admission("information", false)).unwrap();
            claim_information(conn, "receiver", &message.record.id, 43)
                .unwrap()
                .unwrap()
        }
        "reply" => {
            let origin = RequesterOrigin {
                provider: "claude".into(),
                conversation_id: "session-original".into(),
                generation: 2,
            };
            let task =
                admit_with_requester_origin(conn, admission("work", true), Some(&origin)).unwrap();
            let response = reply(
                conn,
                "receiver",
                &task.record.id,
                ReplyStatus::Done,
                "result",
            )
            .unwrap();
            claim_reply_continuation(conn, "sender", &response.record.id, 43)
                .unwrap()
                .unwrap()
        }
        _ => panic!("unknown test operation"),
    }
}

fn claude_mod_native(
    conn: &Connection,
    claim: &TaskClaim,
    phase: crate::native_transport::NativeDeliveryPhase,
) -> crate::native_transport::NativeDeliveryRecord {
    use crate::native_transport::{
        NativeDeliveryRecord, NativeMessageEnvelope, NativeMessageOperation,
    };
    let record = NativeDeliveryRecord {
        envelope: NativeMessageEnvelope {
            interaction_id: claim.record.id.clone(),
            message_id: "message-1".into(),
            target_agent_id: claim.record.target_session_ids[0].clone(),
            sender_agent_id: None,
            parent_interaction_id: None,
            caller_idempotency_key: None,
            generation: 7,
            operation: NativeMessageOperation::StartTurn,
            deadline_at: None,
            body: "prompt".into(),
        },
        canonical_hash: "hash".into(),
        provider: "claude".into(),
        transport: "claude_mod".into(),
        phase,
        provider_request_id: None,
        provider_turn_id: None,
        detail: None,
        created_at: now(),
        updated_at: now(),
    };
    crate::db::upsert_native_delivery_with_conn(conn, &record).unwrap();
    record
}

fn claude_mod_proof(claim: &TaskClaim) -> crate::native_transport::NonSubmissionProof {
    use crate::claude_mod::{ClaudeModLaunchPlan, ClaudeModResolver};
    let temp = tempfile::tempdir().unwrap();
    let plan = ClaudeModLaunchPlan::new(
        temp.path(),
        &claim.record.target_session_ids[0],
        7,
        "session-original",
    )
    .unwrap();
    plan.materialize(&[]).unwrap();
    std::fs::write(
        plan.inbox()
            .join("rejected")
            .join(format!("{}.json", claim.record.id)),
        serde_json::to_vec(&serde_json::json!({
            "generation":7, "interaction_id":claim.record.id,
            "session_id":"session-original", "reason":"invalid payload",
        }))
        .unwrap(),
    )
    .unwrap();
    ClaudeModResolver::open(plan.inbox())
        .unwrap()
        .resolve_rejected(&claim.record.id)
        .unwrap()
        .unwrap()
}

fn claude_mod_mailbox(
    conn: &Connection,
    id: &str,
) -> (String, i64, Option<String>, Option<String>) {
    conn.query_row(
        "SELECT owner,generation,claim_token,continuation_state FROM agent_message_delivery WHERE interaction_id=?1",
        [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap()
}

#[test]
fn claude_mod_proof_recovers_uncertain_with_distinct_scheduler_generation() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for operation in ["followup_task", "send_message", "reply"] {
        let conn = database();
        let claim = claude_mod_claim(&conn, operation);
        claude_mod_native(&conn, &claim, Phase::SubmittedUnconfirmed);
        finish_claim(&conn, &claim, "uncertain").unwrap();
        let proof = claude_mod_proof(&claim);
        assert_ne!(proof.generation(), claim.generation);
        resolve_claude_mod_non_submission_with_conn(&conn, &proof).unwrap();
        let expected_owner = if operation == "followup_task" {
            "pending"
        } else {
            "stored"
        };
        let expected_continuation = (operation == "reply").then(|| "pending".to_owned());
        assert_eq!(
            claude_mod_mailbox(&conn, &claim.record.id),
            (expected_owner.into(), 43, None, expected_continuation)
        );
        assert_eq!(
            crate::db::native_delivery_with_conn(&conn, &claim.record.id)
                .unwrap()
                .unwrap()
                .phase,
            Phase::FailedBeforeSubmit
        );
        resolve_claude_mod_non_submission_with_conn(&conn, &proof).unwrap();
        let recovered = match operation {
            "followup_task" => claim_next_task(&conn, "receiver", 44),
            "send_message" => claim_information(&conn, "receiver", &claim.record.id, 44),
            "reply" => claim_reply_continuation(&conn, "sender", &claim.record.id, 44),
            _ => unreachable!(),
        }
        .unwrap()
        .unwrap();
        assert_ne!(recovered.token, claim.token);
        assert_eq!(recovered.generation, 44);
        assert_eq!(
            finish_claim(&conn, &claim, "provider_accepted")
                .unwrap_err()
                .code,
            "stale_claim"
        );
    }
}

#[test]
fn claude_mod_claim_only_evidence_cannot_recover_the_canonical_mailbox() {
    use crate::claude_mod::{ClaudeModLaunchPlan, ClaudeModResolver, GenerationExitVerifier};
    use crate::native_transport::NativeDeliveryPhase as Phase;
    let conn = database();
    let claim = claude_mod_claim(&conn, "followup_task");
    claude_mod_native(&conn, &claim, Phase::SubmittedUnconfirmed);
    finish_claim(&conn, &claim, "uncertain").unwrap();
    let before = claude_mod_mailbox(&conn, &claim.record.id);
    let temp = tempfile::tempdir().unwrap();
    let plan = ClaudeModLaunchPlan::new(temp.path(), "receiver", 7, "session-original").unwrap();
    plan.materialize(&[]).unwrap();
    std::fs::write(
        plan.inbox()
            .join("claimed")
            .join(format!("{}.json", claim.record.id)),
        b"{torn",
    )
    .unwrap();
    let resolver = ClaudeModResolver::open(plan.inbox()).unwrap();
    struct Exited;
    impl GenerationExitVerifier for Exited {
        fn process_tree_exited(&self, _: &str, _: u64) -> bool {
            true
        }
    }
    assert!(resolver
        .resolve_rejected(&claim.record.id)
        .unwrap()
        .is_none());
    assert!(resolver
        .resolve_after_verified_exit(&claim.record.id, &Exited)
        .unwrap()
        .is_none());
    reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
    assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), before);
    assert!(claim_next_task(&conn, "receiver", 44).unwrap().is_none());
    assert!(receive(&conn, "receiver", None, None, 100)
        .unwrap()
        .messages
        .is_empty());
}

#[test]
fn claude_mod_stale_proof_cannot_rewind_a_replacement_native_attempt() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    let conn = database();
    let claim = claude_mod_claim(&conn, "followup_task");
    let mut replacement = claude_mod_native(&conn, &claim, Phase::SubmittedUnconfirmed);
    finish_claim(&conn, &claim, "uncertain").unwrap();
    let proof = claude_mod_proof(&claim);
    replacement.envelope.generation = 8;
    crate::db::upsert_native_delivery_with_conn(&conn, &replacement).unwrap();
    let before = claude_mod_mailbox(&conn, &claim.record.id);
    assert_eq!(
        resolve_claude_mod_non_submission_with_conn(&conn, &proof)
            .unwrap_err()
            .code,
        "stale_proof"
    );
    assert_eq!(
        crate::db::native_delivery_with_conn(&conn, &claim.record.id)
            .unwrap()
            .unwrap(),
        replacement
    );
    assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), before);
}

#[test]
fn claude_mod_proof_refuses_other_providers_transports_and_recipients() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for mismatch in ["provider", "transport", "recipient"] {
        let conn = database();
        let claim = claude_mod_claim(&conn, "followup_task");
        let mut native = claude_mod_native(&conn, &claim, Phase::SubmittedUnconfirmed);
        finish_claim(&conn, &claim, "uncertain").unwrap();
        let proof = claude_mod_proof(&claim);
        match mismatch {
            "provider" => native.provider = "codex".into(),
            "transport" => native.transport = "terminal".into(),
            "recipient" => native.envelope.target_agent_id = "other-agent".into(),
            _ => unreachable!(),
        }
        crate::db::upsert_native_delivery_with_conn(&conn, &native).unwrap();
        let before = claude_mod_mailbox(&conn, &claim.record.id);
        assert_eq!(
            resolve_claude_mod_non_submission_with_conn(&conn, &proof)
                .unwrap_err()
                .code,
            "stale_proof"
        );
        assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), before);
        assert_eq!(
            crate::db::native_delivery_with_conn(&conn, &claim.record.id)
                .unwrap()
                .unwrap(),
            native
        );
    }
}

#[test]
fn claude_mod_proof_does_not_rewind_positive_or_terminal_native_phases() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for phase in [
        Phase::ProviderAccepted,
        Phase::TurnStarted,
        Phase::Completed,
        Phase::Failed,
        Phase::FailedBeforeSubmit,
    ] {
        let conn = database();
        let claim = claude_mod_claim(&conn, "followup_task");
        let native = claude_mod_native(&conn, &claim, phase);
        finish_claim(&conn, &claim, "uncertain").unwrap();
        let before = claude_mod_mailbox(&conn, &claim.record.id);
        resolve_claude_mod_non_submission_with_conn(&conn, &claude_mod_proof(&claim)).unwrap();
        assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), before);
        assert_eq!(
            crate::db::native_delivery_with_conn(&conn, &claim.record.id)
                .unwrap()
                .unwrap(),
            native
        );
    }
}

#[test]
fn claude_mod_proof_reset_rolls_back_both_records_on_mailbox_failure() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    let conn = database();
    let claim = claude_mod_claim(&conn, "followup_task");
    let native = claude_mod_native(&conn, &claim, Phase::SubmittedUnconfirmed);
    finish_claim(&conn, &claim, "uncertain").unwrap();
    let before = claude_mod_mailbox(&conn, &claim.record.id);
    let proof = claude_mod_proof(&claim);
    conn.execute_batch("CREATE TRIGGER fail_mod_reset BEFORE UPDATE OF owner ON agent_message_delivery WHEN NEW.owner='pending' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(resolve_claude_mod_non_submission_with_conn(&conn, &proof).is_err());
    assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), before);
    assert_eq!(
        crate::db::native_delivery_with_conn(&conn, &claim.record.id)
            .unwrap()
            .unwrap(),
        native
    );
    conn.execute_batch("DROP TRIGGER fail_mod_reset").unwrap();
    resolve_claude_mod_non_submission_with_conn(&conn, &proof).unwrap();
    assert_eq!(claude_mod_mailbox(&conn, &claim.record.id).0, "pending");
}

#[test]
fn claude_mod_proof_does_not_reopen_a_completed_task_or_terminal_owner() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for terminal in [
        "completed_task",
        "failed",
        "provider_accepted",
        "receiver_available",
    ] {
        let conn = database();
        let claim = claude_mod_claim(&conn, "followup_task");
        claude_mod_native(&conn, &claim, Phase::SubmittedUnconfirmed);
        if terminal == "completed_task" {
            finish_claim(&conn, &claim, "uncertain").unwrap();
            reply(
                &conn,
                "receiver",
                &claim.record.id,
                ReplyStatus::Done,
                "done",
            )
            .unwrap();
        } else if terminal == "receiver_available" {
            release_before_write(&conn, &claim).unwrap();
            receive(&conn, "receiver", None, None, 100).unwrap();
        } else {
            finish_claim(&conn, &claim, terminal).unwrap();
        }
        let before = claude_mod_mailbox(&conn, &claim.record.id);
        resolve_claude_mod_non_submission_with_conn(&conn, &claude_mod_proof(&claim)).unwrap();
        assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), before);
        assert!(claim_next_task(&conn, "receiver", 44).unwrap().is_none());
    }
}

#[test]
fn claude_mod_late_positive_receipts_settle_uncertain_without_replay() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for operation in ["followup_task", "send_message", "reply"] {
        for (phase, owner) in [
            (Phase::ProviderAccepted, "provider_accepted"),
            (Phase::TurnStarted, "provider_visible"),
            (Phase::Completed, "provider_completed"),
        ] {
            let conn = database();
            let claim = claude_mod_claim(&conn, operation);
            let native = claude_mod_native(&conn, &claim, phase);
            finish_claim(&conn, &claim, "uncertain").unwrap();
            reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
            reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
            let mailbox = claude_mod_mailbox(&conn, &claim.record.id);
            assert_eq!(mailbox.0, owner);
            assert_eq!(mailbox.1, 43);
            assert_eq!(mailbox.2.as_deref(), Some(claim.token.as_str()));
            if operation == "reply" {
                assert_eq!(mailbox.3.as_deref(), Some("delivered"));
            }
            assert_eq!(
                crate::db::native_delivery_with_conn(&conn, &claim.record.id)
                    .unwrap()
                    .unwrap(),
                native
            );
            assert!(
                receive(&conn, &claim.record.target_session_ids[0], None, None, 100)
                    .unwrap()
                    .messages
                    .is_empty()
            );
        }
    }
}

#[test]
fn claude_mod_late_receipts_do_not_steal_dispatching_or_terminal_owners() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for owner in ["dispatching", "failed", "provider_accepted"] {
        let conn = database();
        let claim = claude_mod_claim(&conn, "reply");
        claude_mod_native(&conn, &claim, Phase::ProviderAccepted);
        if owner != "dispatching" {
            finish_claim(&conn, &claim, owner).unwrap();
        }
        let before = claude_mod_mailbox(&conn, &claim.record.id);
        reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
        assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), before);
    }
}

#[test]
fn claude_mod_dropped_prompts_are_terminal_for_all_canonical_claim_types() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for operation in ["followup_task", "send_message", "reply"] {
        for late in [false, true] {
            let conn = database();
            let claim = claude_mod_claim(&conn, operation);
            claude_mod_native(&conn, &claim, Phase::Failed);
            if late {
                finish_claim(&conn, &claim, "uncertain").unwrap();
                reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
                let settled = claude_mod_mailbox(&conn, &claim.record.id);
                reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
                assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), settled);
            } else {
                finish_claim(&conn, &claim, "failed").unwrap();
            }
            let mailbox = claude_mod_mailbox(&conn, &claim.record.id);
            assert_eq!(mailbox.0, "failed");
            if operation == "reply" {
                assert_eq!(mailbox.3.as_deref(), Some("failed"));
            }
            let recipient = &claim.record.target_session_ids[0];
            assert!(receive(&conn, recipient, None, None, 100)
                .unwrap()
                .messages
                .is_empty());
            assert!(claim_next_task(&conn, recipient, 44).unwrap().is_none());
            assert!(claim_information(&conn, recipient, &claim.record.id, 44)
                .unwrap()
                .is_none());
            assert!(
                claim_reply_continuation(&conn, recipient, &claim.record.id, 44)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                release_before_write(&conn, &claim).unwrap_err().code,
                "stale_claim"
            );
            if operation == "reply" {
                assert!(!set_reply_continuation_state(&conn, &claim.record.id, "pending").unwrap());
                assert!(pending_reply_continuations(&conn, recipient)
                    .unwrap()
                    .is_empty());
            }
            resolve_claude_mod_non_submission_with_conn(&conn, &claude_mod_proof(&claim)).unwrap();
            assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), mailbox);
        }
    }
}

#[test]
fn claude_mod_transcript_advances_accepted_to_visible_without_downgrade() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for operation in ["followup_task", "send_message", "reply"] {
        let conn = database();
        let claim = claude_mod_claim(&conn, operation);
        claude_mod_native(&conn, &claim, Phase::ProviderAccepted);
        finish_claim(&conn, &claim, "uncertain").unwrap();
        reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
        assert_eq!(
            claude_mod_mailbox(&conn, &claim.record.id).0,
            "provider_accepted"
        );
        // A queue acknowledgement alone never projects provider_visible.
        reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
        assert_eq!(
            claude_mod_mailbox(&conn, &claim.record.id).0,
            "provider_accepted"
        );
        claude_mod_native(&conn, &claim, Phase::TurnStarted);
        reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
        let visible = claude_mod_mailbox(&conn, &claim.record.id);
        assert_eq!(visible.0, "provider_visible");
        claude_mod_native(&conn, &claim, Phase::ProviderAccepted);
        reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
        assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), visible);
        claude_mod_native(&conn, &claim, Phase::Completed);
        reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
        assert_eq!(
            claude_mod_mailbox(&conn, &claim.record.id).0,
            "provider_completed"
        );
    }
}

#[test]
fn claude_mod_proof_reset_allows_idempotent_live_release_without_touching_new_claim() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for operation in ["followup_task", "send_message", "reply"] {
        for next_generation in [43, 44] {
            let conn = database();
            let claim = claude_mod_claim(&conn, operation);
            claude_mod_native(&conn, &claim, Phase::SubmittedUnconfirmed);
            // Simulate the waiting live caller: canonical ownership is still dispatching.
            resolve_claude_mod_non_submission_with_conn(&conn, &claude_mod_proof(&claim)).unwrap();
            let cleared = claude_mod_mailbox(&conn, &claim.record.id);
            release_before_write(&conn, &claim).unwrap();
            release_before_write(&conn, &claim).unwrap();
            assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), cleared);
            let next = match operation {
                "followup_task" => claim_next_task(&conn, "receiver", next_generation),
                "send_message" => {
                    claim_information(&conn, "receiver", &claim.record.id, next_generation)
                }
                "reply" => {
                    claim_reply_continuation(&conn, "sender", &claim.record.id, next_generation)
                }
                _ => unreachable!(),
            }
            .unwrap()
            .unwrap();
            let claimed = claude_mod_mailbox(&conn, &claim.record.id);
            assert_eq!(claimed.2.as_deref(), Some(next.token.as_str()));
            assert_eq!(
                release_before_write(&conn, &claim).unwrap_err().code,
                "stale_claim"
            );
            assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), claimed);
        }
    }
}

#[test]
fn claude_mod_idempotent_release_requires_exact_native_evidence_and_scheduler_generation() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for mismatch in [
        "missing",
        "provider",
        "transport",
        "recipient",
        "phase",
        "runtime_generation",
        "scheduler_generation",
    ] {
        let conn = database();
        let claim = claude_mod_claim(&conn, "followup_task");
        release_before_write(&conn, &claim).unwrap();
        if mismatch != "missing" {
            let mut native = claude_mod_native(&conn, &claim, Phase::FailedBeforeSubmit);
            match mismatch {
                "provider" => native.provider = "codex".into(),
                "transport" => native.transport = "terminal".into(),
                "recipient" => native.envelope.target_agent_id = "other-agent".into(),
                "phase" => native.phase = Phase::SubmittedUnconfirmed,
                "runtime_generation" => native.envelope.generation = 0,
                "scheduler_generation" => {
                    conn.execute(
                        "UPDATE agent_message_delivery SET generation=44 WHERE interaction_id=?1",
                        [&claim.record.id],
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            crate::db::upsert_native_delivery_with_conn(&conn, &native).unwrap();
        }
        let before = claude_mod_mailbox(&conn, &claim.record.id);
        assert_eq!(
            release_before_write(&conn, &claim).unwrap_err().code,
            "stale_claim",
            "{mismatch}"
        );
        assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), before);
    }
}

#[test]
fn claude_mod_stronger_receipt_before_queued_caller_settlement_is_not_stranded() {
    use crate::native_transport::NativeDeliveryPhase as Phase;
    for operation in ["followup_task", "send_message", "reply"] {
        for (phase, owner) in [
            (Phase::TurnStarted, "provider_visible"),
            (Phase::Completed, "provider_completed"),
            (Phase::Failed, "failed"),
        ] {
            let conn = database();
            let claim = claude_mod_claim(&conn, operation);
            // The caller has already captured an accepted surface snapshot.
            claude_mod_native(&conn, &claim, Phase::ProviderAccepted);
            let dispatching = claude_mod_mailbox(&conn, &claim.record.id);
            // The observer receives stronger evidence before the caller settles.
            claude_mod_native(&conn, &claim, phase);
            reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
            assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), dispatching);
            // Match finish_agent_task: settle the snapshot, then reconcile the
            // durable native receipt under the application's mutation lock.
            finish_claim(&conn, &claim, "provider_accepted").unwrap();
            reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
            let settled = claude_mod_mailbox(&conn, &claim.record.id);
            assert_eq!(settled.0, owner);
            assert_eq!(settled.1, 43);
            assert_eq!(settled.2.as_deref(), Some(claim.token.as_str()));
            if operation == "reply" {
                let continuation = if phase == Phase::Failed {
                    "failed"
                } else {
                    "delivered"
                };
                assert_eq!(settled.3.as_deref(), Some(continuation));
            }
            reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
            assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), settled);
            // Even an older queued observation must not overwrite stronger state.
            claude_mod_native(&conn, &claim, Phase::ProviderAccepted);
            reconcile_claude_mod_receipt_with_conn(&conn, &claim.record.id).unwrap();
            assert_eq!(claude_mod_mailbox(&conn, &claim.record.id), settled);
            let recipient = &claim.record.target_session_ids[0];
            assert!(receive(&conn, recipient, None, None, 100)
                .unwrap()
                .messages
                .is_empty());
            assert!(claim_next_task(&conn, recipient, 44).unwrap().is_none());
            assert!(claim_information(&conn, recipient, &claim.record.id, 44)
                .unwrap()
                .is_none());
            assert!(
                claim_reply_continuation(&conn, recipient, &claim.record.id, 44)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                release_before_write(&conn, &claim).unwrap_err().code,
                "stale_claim"
            );
        }
    }
}
