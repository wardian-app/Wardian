//! Exact scheduler admissions and a durable terminal outbox. Never replay provider work.
use super::*;

/// Host observation of accepted task input, separate from a turn-start acknowledgement.
/// The native adapter assembles this from existing provider input/source records.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskDeliveryReceipt {
    pub generation: u64,
    pub provider_session_id: String,
    pub provider_turn_id: String,
    pub source: TaskReceiptSource,
}

/// Only literal accepted input or an answer to an exactly issued current call
/// can authorize automatic completion. Synthetic, uncorrelated outputs cannot.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskReceiptSource {
    AcceptedUserMessage {
        source_id: String,
        task_body_sha256: String,
    },
    ToolCallOutput {
        call_id: String,
        issued_generation: u64,
        issued_thread_id: String,
        task_body_sha256: String,
    },
}

/// Durable identity of one scheduler task admitted to an exact native Codex turn.
/// Multiple requests may share a turn; each retains its own claim and requester.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTurnBinding {
    pub request_id: String,
    pub claim_token: String,
    pub recipient: String,
    pub generation: u64,
    pub provider: String,
    pub provider_session_id: String,
    pub provider_turn_id: String,
    pub admission_mode: String,
}

pub(super) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS agent_message_task_turns (
        request_id TEXT PRIMARY KEY NOT NULL,
        claim_token TEXT NOT NULL,
        recipient TEXT NOT NULL,
        generation INTEGER NOT NULL,
        provider TEXT NOT NULL CHECK(provider='codex'),
        provider_session_id TEXT NOT NULL,
        provider_turn_id TEXT NOT NULL,
        admission_mode TEXT NOT NULL CHECK(admission_mode IN ('start','steer')),
        settlement TEXT NOT NULL CHECK(settlement IN ('bound','outcome_recorded','published','uncertain')),
        terminal_status TEXT CHECK(terminal_status IN ('done','failed','blocked')),
        terminal_body TEXT CHECK(length(CAST(terminal_body AS BLOB)) <= {MAX_MESSAGE_BYTES}),
        CHECK((settlement IN ('bound','uncertain') AND terminal_status IS NULL AND terminal_body IS NULL)
           OR (settlement IN ('outcome_recorded','published') AND terminal_status IS NOT NULL AND terminal_body IS NOT NULL)));
        CREATE INDEX IF NOT EXISTS agent_message_task_turn_settlement
        ON agent_message_task_turns(settlement);"
    ))?;
    let has_receipt = conn
        .prepare("PRAGMA table_info(agent_message_task_turns)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|column| column == "delivery_receipt");
    if !has_receipt {
        // Historical bindings gain no authority during migration.
        conn.execute_batch(
            "ALTER TABLE agent_message_task_turns ADD COLUMN delivery_receipt TEXT",
        )?;
    }
    Ok(())
}

fn validate_receipt(
    conn: &Connection,
    binding: &TaskTurnBinding,
    receipt: &TaskDeliveryReceipt,
) -> Result<()> {
    let task = load(conn, &binding.request_id)?;
    let InteractionBodyRef::Inline { body } = task.body_ref else {
        return Err(Error::new(
            "invalid_task",
            "Task receipt requires canonical inline input.",
        ));
    };
    let expected = format!("{:x}", Sha256::digest(body.as_bytes()));
    let valid_source = match &receipt.source {
        TaskReceiptSource::AcceptedUserMessage {
            source_id,
            task_body_sha256,
        } => !source_id.trim().is_empty() && task_body_sha256 == &expected,
        TaskReceiptSource::ToolCallOutput {
            call_id,
            issued_generation,
            issued_thread_id,
            task_body_sha256,
        } => {
            !call_id.trim().is_empty()
                && *issued_generation == binding.generation
                && issued_thread_id == &binding.provider_session_id
                && task_body_sha256 == &expected
        }
    };
    if receipt.generation != binding.generation
        || receipt.provider_session_id != binding.provider_session_id
        || receipt.provider_turn_id != binding.provider_turn_id
        || !valid_source
    {
        return Err(Error::new(
            "invalid_delivery_receipt",
            "Task input receipt does not match this task, generation, thread and turn.",
        ));
    }
    Ok(())
}

/// Persist a native adapter's exact accepted-input observation. Callers must
/// prove source identity and submitted bytes before constructing this receipt.
/// Repeated observations are idempotent; conflicting evidence fails closed.
pub fn record_task_delivery_receipt(
    conn: &Connection,
    binding: &TaskTurnBinding,
    receipt: &TaskDeliveryReceipt,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let state = settlement(&tx, binding)?;
    validate_receipt(&tx, binding, receipt)?;
    let value = serde_json::to_string(receipt)
        .map_err(|e| Error::new("invalid_delivery_receipt", e.to_string()))?;
    let previous: Option<String> = tx.query_row(
        "SELECT delivery_receipt FROM agent_message_task_turns WHERE request_id=?1",
        [&binding.request_id],
        |row| row.get(0),
    )?;
    if previous.as_ref().is_some_and(|old| old != &value) {
        return Err(Error::new(
            "conflicting_delivery_receipt",
            "Task already has a different input receipt.",
        ));
    }
    if previous.as_deref() == Some(value.as_str()) {
        return Ok(());
    }
    if state != "bound" {
        return Err(Error::new(
            "invalid_state",
            "Task input receipt must precede its outcome.",
        ));
    }
    tx.execute(
        "UPDATE agent_message_task_turns SET delivery_receipt=?2 WHERE request_id=?1",
        params![binding.request_id, value],
    )?;
    tx.commit()?;
    Ok(())
}

/// Atomically bind a still-dispatching v2 claim and finish accepted delivery.
/// An explicit reply may already be terminal; its existence does not revoke admission.
/// Call only on an exact native Codex acceptance receipt, never inferred continuity.
pub fn bind_task_turn(
    conn: &Connection,
    claim: &TaskClaim,
    provider: &str,
    session: &str,
    turn: &str,
    mode: &str,
) -> Result<TaskTurnBinding> {
    if provider != "codex"
        || session.trim().is_empty()
        || turn.trim().is_empty()
        || !matches!(mode, "start" | "steer")
    {
        return Err(Error::new(
            "invalid_binding",
            "Exact Codex session, turn, and admission mode are required.",
        ));
    }
    let tx = conn.unchecked_transaction()?;
    if !owns_claim(&tx, claim)? {
        return Err(Error::new(
            "stale_claim",
            "Task is no longer dispatching under this claim.",
        ));
    }
    let task = load(&tx, &claim.record.id)?;
    let recipient: String = tx.query_row(
        "SELECT recipient FROM agent_message_delivery WHERE interaction_id=?1 AND operation='followup_task'",
        [&task.id], |row| row.get(0),
    ).optional()?.ok_or_else(|| Error::new("invalid_task", "Binding requires a v2 task."))?;
    if task.kind != InteractionKind::Task
        || claim.record.kind != InteractionKind::Task
        || task.target_session_ids != [recipient.clone()]
        || claim.record.target_session_ids != task.target_session_ids
        || (task.sender_session_id.is_none()
            && host_automation_provenance(&tx, &task.id)?.is_none())
    {
        return Err(Error::new(
            "invalid_task",
            "Claim does not identify the canonical task recipient and provenance.",
        ));
    }
    if let Some(prepared) = recipient_origin(&tx, &task.id)? {
        if prepared.provider != provider
            || prepared.conversation_id != session
            || prepared.generation != claim.generation
        {
            return Err(Error::new(
                "invalid_binding",
                "Acknowledged Codex thread differs from the prepared task root.",
            ));
        }
    }
    let binding = TaskTurnBinding {
        request_id: task.id,
        claim_token: claim.token.clone(),
        recipient,
        generation: claim.generation,
        provider: provider.into(),
        provider_session_id: session.into(),
        provider_turn_id: turn.into(),
        admission_mode: mode.into(),
    };
    tx.execute(
        "INSERT INTO agent_message_task_turns(request_id,claim_token,recipient,generation,provider,provider_session_id,provider_turn_id,admission_mode,settlement)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'bound')",
        params![binding.request_id, binding.claim_token, binding.recipient, binding.generation,
            binding.provider, binding.provider_session_id, binding.provider_turn_id, binding.admission_mode],
    )?;
    finish_claim(&tx, claim, "provider_accepted")?;
    tx.commit()?;
    Ok(binding)
}

fn row_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskTurnBinding> {
    Ok(TaskTurnBinding {
        request_id: row.get(0)?,
        claim_token: row.get(1)?,
        recipient: row.get(2)?,
        generation: row.get(3)?,
        provider: row.get(4)?,
        provider_session_id: row.get(5)?,
        provider_turn_id: row.get(6)?,
        admission_mode: row.get(7)?,
    })
}

/// Validate persisted binding and delivery ownership; no live runtime generation is consulted.
/// The observation caller must establish current runtime generation before recording.
fn settlement(conn: &Connection, binding: &TaskTurnBinding) -> Result<String> {
    let state = conn.query_row(
        "SELECT b.settlement FROM agent_message_task_turns b
        JOIN agent_message_delivery d ON d.interaction_id=b.request_id
        WHERE b.request_id=?1 AND b.claim_token=?2 AND b.recipient=?3 AND b.generation=?4
          AND b.provider=?5 AND b.provider_session_id=?6 AND b.provider_turn_id=?7 AND b.admission_mode=?8
          AND d.claim_token=b.claim_token AND d.generation=b.generation AND d.recipient=b.recipient
          AND d.operation='followup_task' AND d.owner='provider_accepted'",
        params![binding.request_id, binding.claim_token, binding.recipient, binding.generation,
            binding.provider, binding.provider_session_id, binding.provider_turn_id, binding.admission_mode],
        |row| row.get(0),
    ).optional()?.ok_or_else(|| Error::new("stale_binding", "Task turn binding or accepted claim no longer matches."))?;
    let task = load(conn, &binding.request_id)?;
    if task.kind != InteractionKind::Task || task.target_session_ids != [binding.recipient.clone()]
    {
        return Err(Error::new(
            "invalid_task",
            "Binding does not identify the canonical task recipient.",
        ));
    }
    Ok(state)
}

/// Commit known terminal output before attempting reply publication. No provider work occurs.
/// Identical observations are idempotent; the first committed terminal output is retained.
pub fn record_task_turn_outcome(
    conn: &Connection,
    binding: &TaskTurnBinding,
    status: ReplyStatus,
    body: &str,
) -> Result<()> {
    validate_message(body)?;
    let tx = conn.unchecked_transaction()?;
    let state = settlement(&tx, binding)?;
    let status = super::super::enum_value(&status)?;
    match state.as_str() {
        "bound" => {
            tx.execute("UPDATE agent_message_task_turns SET settlement='outcome_recorded',terminal_status=?2,terminal_body=?3 WHERE request_id=?1",
                params![binding.request_id, status, body])?;
        }
        "outcome_recorded" | "published" => {
            let same: bool = tx.query_row(
                "SELECT terminal_status=?2 AND terminal_body=?3 FROM agent_message_task_turns WHERE request_id=?1",
                params![binding.request_id, status, body], |row| row.get(0),
            )?;
            if !same {
                return Err(Error::new(
                    "conflicting_outcome",
                    "Task turn already has a different terminal outcome.",
                ));
            }
        }
        _ => {
            return Err(Error::new(
                "uncertain_binding",
                "Lost turn continuity cannot authorize an outcome.",
            ))
        }
    }
    tx.commit()?;
    Ok(())
}

/// Publish only persisted output using the canonical atomic reply transaction.
/// Any committed explicit reply wins, regardless of its status/body. A duplicate
/// publishes no availability and returns None. Conflicting later explicit replies
/// continue to use the existing explicit-reply error semantics.
pub fn publish_task_turn_outcome(
    conn: &Connection,
    binding: &TaskTurnBinding,
) -> Result<Option<Replied>> {
    let tx = conn.unchecked_transaction()?;
    if settlement(&tx, binding)? != "outcome_recorded" {
        return Ok(None);
    }
    let previous: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM structured_replies WHERE request_id=?1 AND status IN ('done','failed','blocked'))",
        [&binding.request_id], |row| row.get(0),
    )?;
    let replied = if previous {
        None
    } else {
        let (status, body): (String, String) = tx.query_row(
            "SELECT terminal_status,terminal_body FROM agent_message_task_turns WHERE request_id=?1",
            [&binding.request_id], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        validate_message(&body)?;
        let receipt: Option<String> = tx.query_row(
            "SELECT delivery_receipt FROM agent_message_task_turns WHERE request_id=?1",
            [&binding.request_id],
            |row| row.get(0),
        )?;
        let receipted = receipt
            .as_deref()
            .and_then(|value| serde_json::from_str::<TaskDeliveryReceipt>(value).ok())
            .is_some_and(|receipt| validate_receipt(&tx, binding, &receipt).is_ok());
        if receipted {
            Some(reply_in_transaction(
                &tx,
                &binding.recipient,
                &binding.request_id,
                super::super::enum_from_value(&status)?,
                &body,
                None,
            )?)
        } else {
            publish_unreceipted_information(&tx, binding, &body)?;
            None
        }
    };
    tx.execute(
        "UPDATE agent_message_task_turns SET settlement='published' WHERE request_id=?1",
        [&binding.request_id],
    )?;
    tx.commit()?;
    Ok(replied)
}

/// Keep a visible result without assigning terminal authority to an unrelated turn.
/// This and outbox settlement share a transaction, including on restart recovery.
fn publish_unreceipted_information(
    conn: &Connection,
    binding: &TaskTurnBinding,
    body: &str,
) -> Result<()> {
    use crate::control::{DeliveryTransportKind, InteractionDeliveryAttemptRecord};
    let diagnostic = "Delivered without receipt; awaiting explicit reply or redelivery. Automatic final output is informational and this task remains open.";
    let timestamp = now();
    super::super::upsert_interaction_delivery_attempt_with_conn(
        conn,
        &InteractionDeliveryAttemptRecord {
            id: format!("task-receipt:{}", binding.request_id),
            interaction_id: binding.request_id.clone(),
            target_session_id: binding.recipient.clone(),
            transport: DeliveryTransportKind::NativeProvider,
            generation: binding.generation,
            runtime_state: "turn_completed".into(),
            delivery_state: "delivered_without_receipt".into(),
            delivery_phase: Some("terminal_publication".into()),
            observed_state: None,
            reason: Some(diagnostic.into()),
            error: None,
            created_at: timestamp.clone(),
            updated_at: timestamp.clone(),
        },
    )?;
    let task = load(conn, &binding.request_id)?;
    if let Some(recipient) = task.sender_session_id {
        // Send the actionable notice separately so the final remains exact,
        // including at the message byte limit. Both are information, not replies.
        let origin = requester_origin(conn, &binding.request_id)?;
        for (id, message) in [
            (
                format!("int_unreceipted_notice_{}", binding.request_id),
                format!("Wardian task {}: {diagnostic}", binding.request_id),
            ),
            (
                format!("int_unreceipted_{}", binding.request_id),
                body.to_owned(),
            ),
        ] {
            let record = InteractionRecord {
                id,
                kind: InteractionKind::Message,
                sender_session_id: Some(binding.recipient.clone()),
                target_session_ids: vec![recipient.clone()],
                status: InteractionStatus::Queued,
                trigger_policy: InteractionTriggerPolicy::NotifyOnly,
                body_ref: InteractionBodyRef::Inline { body: message },
                parent_interaction_id: Some(binding.request_id.clone()),
                created_at: timestamp.clone(),
                updated_at: timestamp.clone(),
                completed_at: None,
            };
            super::super::upsert_interaction_record_with_conn(conn, &record)?;
            conn.execute("INSERT INTO agent_message_delivery(interaction_id,sender,recipient,operation,fingerprint,owner,generation,wake_on_admission,recipient_provider,recipient_conversation_id,recipient_generation)
            VALUES(?1,?2,?3,'send_message','','stored',0,?4,?5,?6,?7)",
            params![record.id, binding.recipient, recipient, origin.as_ref().map(|_| 1_i64),
                origin.as_ref().map(|o| o.provider.as_str()), origin.as_ref().map(|o| o.conversation_id.as_str()),
                origin.as_ref().map(|o| o.generation)])?;
            make_available(conn, &recipient, &record.id)?;
        }
    }
    Ok(())
}

/// Read the informational projection committed by the terminal outbox, if any.
/// The deterministic identity makes runtime cache publication replay-safe.
pub fn task_turn_information(
    conn: &Connection,
    binding: &TaskTurnBinding,
) -> Result<Vec<InteractionRecord>> {
    settlement(conn, binding)?;
    let mut records = Vec::new();
    for id in [
        format!("int_unreceipted_notice_{}", binding.request_id),
        format!("int_unreceipted_{}", binding.request_id),
    ] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM interactions WHERE id=?1)",
            [&id],
            |row| row.get(0),
        )?;
        if exists {
            records.push(load(conn, &id)?);
        }
    }
    Ok(records)
}

fn bindings_in_state(conn: &Connection, state: &str) -> Result<Vec<TaskTurnBinding>> {
    let mut statement = conn.prepare(
        "SELECT request_id,claim_token,recipient,generation,provider,provider_session_id,provider_turn_id,admission_mode
        FROM agent_message_task_turns WHERE settlement=?1 ORDER BY request_id",
    )?;
    let rows = statement.query_map([state], row_binding)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// List unresolved admissions for exact continuity checks or marking uncertain.
/// Recorded outcomes have a separate recovery path; uncertain turns are never retried.
pub fn pending_task_turn_bindings(conn: &Connection) -> Result<Vec<TaskTurnBinding>> {
    bindings_in_state(conn, "bound")
}

/// Process-start hydrate retires only observations without a recorded outcome.
/// Live receive recovery must not retire a turn that the runtime still observes.
pub fn abandon_task_turn_observations(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE agent_message_task_turns SET settlement='uncertain' WHERE settlement='bound' AND terminal_status IS NULL AND terminal_body IS NULL", [])?;
    Ok(())
}

/// Retire an admission whose exact provider continuity was lost, without rerunning
/// or reconstructing it. Known outcomes must instead remain recoverable.
pub fn mark_task_turn_uncertain(conn: &Connection, binding: &TaskTurnBinding) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    match settlement(&tx, binding)?.as_str() {
        "bound" => {
            tx.execute(
                "UPDATE agent_message_task_turns SET settlement='uncertain' WHERE request_id=?1",
                [&binding.request_id],
            )?;
        }
        "uncertain" => {}
        _ => {
            return Err(Error::new(
                "invalid_state",
                "Known outcomes cannot be discarded as uncertain.",
            ))
        }
    }
    tx.commit()?;
    Ok(())
}

/// Restart recovery publishes only previously committed outcomes. It requires
/// neither current runtime ownership nor any provider execution or reconstruction.
pub fn recover_task_turn_outcomes(conn: &Connection) -> Result<Vec<Replied>> {
    let mut replies = Vec::new();
    for binding in bindings_in_state(conn, "outcome_recorded")? {
        if let Some(replied) = publish_task_turn_outcome(conn, &binding)? {
            replies.push(replied);
        }
    }
    Ok(replies)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn
    }

    fn task(conn: &Connection, sender: &str) -> TaskClaim {
        admit(
            conn,
            Admission {
                sender,
                recipient: "receiver",
                message: "work",
                idempotency_key: None,
                task: true,
                generation: 7,
            },
        )
        .unwrap();
        claim_next_task(conn, "receiver", 7).unwrap().unwrap()
    }

    fn bind(conn: &Connection, claim: &TaskClaim) -> TaskTurnBinding {
        let binding = bind_task_turn(conn, claim, "codex", "session", "turn", "start").unwrap();
        accepted_input(conn, &binding);
        binding
    }

    fn accepted_input(conn: &Connection, binding: &TaskTurnBinding) {
        record_task_delivery_receipt(
            conn,
            binding,
            &TaskDeliveryReceipt {
                generation: binding.generation,
                provider_session_id: binding.provider_session_id.clone(),
                provider_turn_id: binding.provider_turn_id.clone(),
                source: TaskReceiptSource::AcceptedUserMessage {
                    source_id: format!("input:{}", binding.request_id),
                    task_body_sha256: format!("{:x}", Sha256::digest(b"work")),
                },
            },
        )
        .unwrap();
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn conversation_reset_fences_old_context_without_discarding_tasks_or_results() {
        let conn = database();
        let old_information = admit(
            &conn,
            Admission {
                sender: "sender",
                recipient: "receiver",
                message: "old information",
                idempotency_key: None,
                task: false,
                generation: 7,
            },
        )
        .unwrap();
        let lost = bind(&conn, &task(&conn, "sender"));
        let captured = bind(&conn, &task(&conn, "sender"));
        record_task_turn_outcome(&conn, &captured, ReplyStatus::Done, "captured result").unwrap();
        let unclaimed = admit(
            &conn,
            Admission {
                sender: "sender",
                recipient: "receiver",
                message: "pending task",
                idempotency_key: None,
                task: true,
                generation: 7,
            },
        )
        .unwrap();
        let outbound = admit(
            &conn,
            Admission {
                sender: "receiver",
                recipient: "sender",
                message: "outbound task",
                idempotency_key: None,
                task: true,
                generation: 7,
            },
        )
        .unwrap();
        let explicit = reply(
            &conn,
            "sender",
            &outbound.record.id,
            ReplyStatus::Done,
            "explicit result",
        )
        .unwrap();
        fence_conversation_deliveries(&conn, "receiver").unwrap();
        assert!(pending_information(
            &conn,
            "receiver",
            information_highwater(&conn, "receiver").unwrap()
        )
        .unwrap()
        .is_empty());
        assert_eq!(settlement(&conn, &lost).unwrap(), "uncertain");
        assert_eq!(settlement(&conn, &captured).unwrap(), "outcome_recorded");
        assert_eq!(
            load(&conn, &old_information.record.id).unwrap(),
            old_information.record
        );
        assert_eq!(load(&conn, &explicit.record.id).unwrap(), explicit.record);
        let reply_owner: String = conn
            .query_row(
                "SELECT owner FROM agent_message_delivery WHERE interaction_id=?1",
                [&explicit.record.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reply_owner, "blocked_stale_origin");
        assert_eq!(
            claim_next_task(&conn, "receiver", 8)
                .unwrap()
                .unwrap()
                .record
                .id,
            unclaimed.record.id
        );
        assert_eq!(recover_task_turn_outcomes(&conn).unwrap().len(), 1);
        let fresh = admit(
            &conn,
            Admission {
                sender: "sender",
                recipient: "receiver",
                message: "fresh information",
                idempotency_key: None,
                task: false,
                generation: 8,
            },
        )
        .unwrap();
        assert_eq!(
            pending_information(
                &conn,
                "receiver",
                information_highwater(&conn, "receiver").unwrap()
            )
            .unwrap(),
            vec![fresh.record.id]
        );
    }

    #[test]
    fn unreceipted_native_final_leaves_task_awaiting_reply() {
        let conn = database();
        let claim = task(&conn, "sender");
        // A turn/start acknowledgement identifies a turn, but does not prove
        // this task was received as input or answered a current wait call.
        let binding = bind_task_turn(
            &conn,
            &claim,
            "codex",
            "retained-thread",
            "new-turn",
            "start",
        )
        .unwrap();
        record_task_turn_outcome(
            &conn,
            &binding,
            ReplyStatus::Done,
            "answer to an older message",
        )
        .unwrap();
        let _ = publish_task_turn_outcome(&conn, &binding).unwrap();
        assert_eq!(
            load(&conn, &claim.record.id).unwrap().status,
            InteractionStatus::AwaitingReply,
            "An unreceipted final answer must not complete the new task"
        );
        assert_eq!(count(&conn, "structured_replies"), 0);
        let page = receive(&conn, "sender", None, None, 100).unwrap();
        assert_eq!(page.messages.len(), 2);
        assert!(page.messages[0].message.contains(&binding.request_id));
        assert!(page.messages[0]
            .message
            .contains("awaiting explicit reply or redelivery"));
        assert_eq!(page.messages[0].kind, InteractionKind::Message);
        assert_eq!(
            page.messages[0].parent_interaction_id.as_deref(),
            Some(binding.request_id.as_str())
        );
        assert_eq!(page.messages[1].message, "answer to an older message");
        assert_eq!(page.messages[0].reply_status, None);
        let diagnostics =
            crate::db::list_interaction_delivery_attempts_with_conn(&conn, &binding.request_id)
                .unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].delivery_state, "delivered_without_receipt");
        assert!(diagnostics[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("awaiting explicit reply or redelivery"));
        assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
        assert_eq!(
            receive(&conn, "sender", None, None, 100)
                .unwrap()
                .messages
                .len(),
            2
        );
        // Explicit correlation remains sufficient after an informational final.
        reply(
            &conn,
            "receiver",
            &binding.request_id,
            ReplyStatus::Done,
            "explicit result",
        )
        .unwrap();
        assert_eq!(
            load(&conn, &binding.request_id).unwrap().status,
            InteractionStatus::Completed
        );
    }

    #[test]
    fn unreceipted_notice_and_maximum_output_follow_only_the_original_requester_conversation() {
        let conn = database();
        let origin = RequesterOrigin {
            provider: "claude".into(),
            conversation_id: "requester-thread".into(),
            generation: 4,
        };
        let task = admit_with_requester_origin(
            &conn,
            Admission {
                sender: "sender",
                recipient: "receiver",
                message: "work",
                idempotency_key: None,
                task: true,
                generation: 7,
            },
            Some(&origin),
        )
        .unwrap();
        let claim = claim_next_task(&conn, "receiver", 7).unwrap().unwrap();
        let binding = bind_task_turn(&conn, &claim, "codex", "thread", "turn", "start").unwrap();
        let body = "é".repeat(MAX_MESSAGE_BYTES / 2);
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, &body).unwrap();
        assert!(publish_task_turn_outcome(&conn, &binding)
            .unwrap()
            .is_none());
        let information = task_turn_information(&conn, &binding).unwrap();
        assert_eq!(information.len(), 2);
        assert_eq!(
            pending_wakeable_information(&conn, "sender").unwrap().len(),
            2
        );
        for record in &information {
            assert_eq!(record.kind, InteractionKind::Message);
            assert_eq!(
                record.parent_interaction_id.as_deref(),
                Some(task.record.id.as_str())
            );
            assert_eq!(
                recipient_origin(&conn, &record.id).unwrap(),
                Some(RecipientOrigin {
                    provider: origin.provider.clone(),
                    conversation_id: origin.conversation_id.clone(),
                    generation: origin.generation,
                })
            );
        }
        let InteractionBodyRef::Inline { body: published } = &information[1].body_ref else {
            panic!("inline output")
        };
        assert_eq!(published.as_bytes(), body.as_bytes());
        assert_eq!(count(&conn, "structured_replies"), 0);
        assert_eq!(
            load(&conn, &task.record.id).unwrap().status,
            InteractionStatus::AwaitingReply
        );
        fence_conversation_deliveries(&conn, "sender").unwrap();
        assert!(pending_wakeable_information(&conn, "sender")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn current_wait_call_receipt_authorizes_but_old_generation_thread_and_body_do_not() {
        for mismatch in 0..8 {
            let conn = database();
            let claim = task(&conn, "sender");
            let binding =
                bind_task_turn(&conn, &claim, "codex", "thread", "turn", "start").unwrap();
            let mut receipt = TaskDeliveryReceipt {
                generation: 7,
                provider_session_id: "thread".into(),
                provider_turn_id: "turn".into(),
                source: TaskReceiptSource::ToolCallOutput {
                    call_id: "issued-current-wait-call".into(),
                    issued_generation: 7,
                    issued_thread_id: "thread".into(),
                    task_body_sha256: format!("{:x}", Sha256::digest(b"work")),
                },
            };
            match mismatch {
                0 => receipt.generation = 6,
                1 => receipt.provider_session_id = "retained".into(),
                2 => receipt.provider_turn_id = "old-turn".into(),
                _ => {
                    if let TaskReceiptSource::ToolCallOutput {
                        call_id,
                        issued_generation,
                        issued_thread_id,
                        task_body_sha256,
                    } = &mut receipt.source
                    {
                        match mismatch {
                            3 => *issued_generation = 6,
                            4 => *issued_thread_id = "retained".into(),
                            5 => call_id.clear(),
                            6 => task_body_sha256.clear(),
                            _ => {}
                        }
                    }
                }
            }
            let recorded = record_task_delivery_receipt(&conn, &binding, &receipt);
            assert_eq!(recorded.is_ok(), mismatch == 7);
            record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "result").unwrap();
            assert_eq!(
                publish_task_turn_outcome(&conn, &binding)
                    .unwrap()
                    .is_some(),
                mismatch == 7
            );
            assert_eq!(count(&conn, "structured_replies"), i64::from(mismatch == 7));
        }
    }

    #[test]
    fn persisted_receipt_is_revalidated_during_outbox_recovery() {
        let conn = database();
        let binding = bind(&conn, &task(&conn, "sender"));
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "result").unwrap();
        conn.execute(
            "UPDATE agent_message_task_turns SET delivery_receipt='{}' WHERE request_id=?1",
            [&binding.request_id],
        )
        .unwrap();
        assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
        assert_eq!(
            load(&conn, &binding.request_id).unwrap().status,
            InteractionStatus::AwaitingReply
        );
        assert_eq!(count(&conn, "structured_replies"), 0);
    }

    #[test]
    fn binding_requires_exact_dispatching_task_and_native_receipt() {
        let conn = database();
        let claim = task(&conn, "sender");
        for (provider, session, turn, mode) in [
            ("claude", "session", "turn", "start"),
            ("Codex", "session", "turn", "start"),
            ("codex", " ", "turn", "start"),
            ("codex", "session", "", "start"),
            ("codex", "session", "turn", "manual"),
            ("codex", "session", "turn", "Start"),
        ] {
            assert_eq!(
                bind_task_turn(&conn, &claim, provider, session, turn, mode)
                    .unwrap_err()
                    .code,
                "invalid_binding"
            );
        }
        for mismatch in 0..5 {
            let mut bad = TaskClaim {
                record: claim.record.clone(),
                token: claim.token.clone(),
                generation: claim.generation,
            };
            match mismatch {
                0 => bad.token.push('x'),
                1 => bad.generation += 1,
                2 => bad.record.target_session_ids = vec!["other".into()],
                3 => bad.record.id.push('x'),
                _ => bad.record.kind = InteractionKind::Message,
            }
            assert!(bind_task_turn(&conn, &bad, "codex", "session", "turn", "start").is_err());
        }
        assert!(owns_claim(&conn, &claim).unwrap());
        assert_eq!(count(&conn, "agent_message_task_turns"), 0);
        let binding = bind(&conn, &claim);
        assert_eq!(binding.recipient, "receiver");
        assert_eq!(binding.generation, 7);
        assert_eq!(binding.claim_token, claim.token);
        assert!(!owns_claim(&conn, &claim).unwrap());
        assert!(bind_task_turn(&conn, &claim, "codex", "session", "other-turn", "steer").is_err());
        assert!(release_before_write(&conn, &claim).is_err());
        assert!(claim_next_task(&conn, "receiver", 99).unwrap().is_none());
        assert!(receive(&conn, "receiver", None, None, 100)
            .unwrap()
            .messages
            .is_empty());
    }

    #[test]
    fn receiver_information_historical_and_finished_claims_cannot_bind() {
        let conn = database();
        let claim = task(&conn, "sender");
        release_before_write(&conn, &claim).unwrap();
        receive(&conn, "receiver", None, None, 100).unwrap();
        assert!(bind_task_turn(&conn, &claim, "codex", "session", "turn", "start").is_err());
        admit(
            &conn,
            Admission {
                sender: "sender",
                recipient: "receiver",
                message: "info",
                idempotency_key: None,
                task: false,
                generation: 7,
            },
        )
        .unwrap();
        let info = pending_information(
            &conn,
            "receiver",
            information_highwater(&conn, "receiver").unwrap(),
        )
        .unwrap()
        .pop()
        .unwrap();
        let info_claim = claim_information(&conn, "receiver", &info, 7)
            .unwrap()
            .unwrap();
        assert!(bind_task_turn(&conn, &info_claim, "codex", "session", "turn", "start").is_err());
        let historical = task(&conn, "sender");
        conn.execute(
            "DELETE FROM agent_message_delivery WHERE interaction_id=?1",
            [&historical.record.id],
        )
        .unwrap();
        assert!(bind_task_turn(&conn, &historical, "codex", "session", "turn", "start").is_err());
        for state in ["uncertain", "provider_accepted", "failed_before_submit"] {
            let claim = task(&conn, "sender");
            finish_claim(&conn, &claim, state).unwrap();
            assert!(bind_task_turn(&conn, &claim, "codex", "session", "turn", "start").is_err());
        }
        migrate(&conn).unwrap();
        assert_eq!(count(&conn, "agent_message_task_turns"), 0);
        assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
    }

    #[test]
    fn binding_and_acceptance_commit_together() {
        let conn = database();
        let claim = task(&conn, "sender");
        conn.execute_batch("CREATE TRIGGER fail_accept BEFORE UPDATE ON agent_message_delivery WHEN NEW.owner='provider_accepted' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(bind_task_turn(&conn, &claim, "codex", "session", "turn", "start").is_err());
        assert_eq!(count(&conn, "agent_message_task_turns"), 0);
        assert!(owns_claim(&conn, &claim).unwrap());
        conn.execute_batch("DROP TRIGGER fail_accept").unwrap();
        bind(&conn, &claim);
    }

    #[test]
    fn every_binding_identity_and_persisted_claim_tuple_is_validated() {
        let conn = database();
        let binding = bind(&conn, &task(&conn, "sender"));
        for mismatch in 0..8 {
            let mut bad = binding.clone();
            match mismatch {
                0 => bad.request_id.push('x'),
                1 => bad.claim_token.push('x'),
                2 => bad.recipient.push('x'),
                3 => bad.generation += 1,
                4 => bad.provider.push('x'),
                5 => bad.provider_session_id.push('x'),
                6 => bad.provider_turn_id.push('x'),
                _ => bad.admission_mode = "steer".into(),
            }
            assert_eq!(
                record_task_turn_outcome(&conn, &bad, ReplyStatus::Done, "done")
                    .unwrap_err()
                    .code,
                "stale_binding"
            );
            assert!(publish_task_turn_outcome(&conn, &bad).is_err());
            assert!(mark_task_turn_uncertain(&conn, &bad).is_err());
        }
        for (column, value) in [
            ("claim_token", "wrong"),
            ("generation", "9"),
            ("recipient", "wrong"),
            ("owner", "dispatching"),
            ("operation", "send_message"),
        ] {
            let conn = database();
            let binding = bind(&conn, &task(&conn, "sender"));
            conn.execute(
                &format!("UPDATE agent_message_delivery SET {column}=?2 WHERE interaction_id=?1"),
                params![binding.request_id, value],
            )
            .unwrap();
            assert_eq!(
                record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "done")
                    .unwrap_err()
                    .code,
                "stale_binding"
            );
            assert!(publish_task_turn_outcome(&conn, &binding).is_err());
            assert!(mark_task_turn_uncertain(&conn, &binding).is_err());
        }
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "done").unwrap();
        assert!(publish_task_turn_outcome(&conn, &binding)
            .unwrap()
            .is_some());
    }

    #[test]
    fn all_explicit_terminal_statuses_win_before_binding_or_publication() {
        for status in [ReplyStatus::Done, ReplyStatus::Failed, ReplyStatus::Blocked] {
            for ordering in 0..3 {
                let conn = database();
                let claim = task(&conn, "sender");
                let explicit = || {
                    reply(
                        &conn,
                        "receiver",
                        &claim.record.id,
                        status.clone(),
                        "explicit",
                    )
                    .unwrap()
                };
                let first = (ordering == 0).then(explicit);
                let binding = bind(&conn, &claim);
                let first = first.or_else(|| (ordering == 1).then(explicit));
                record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "automatic").unwrap();
                let first = first.unwrap_or_else(explicit);
                assert!(publish_task_turn_outcome(&conn, &binding)
                    .unwrap()
                    .is_none());
                assert!(publish_task_turn_outcome(&conn, &binding)
                    .unwrap()
                    .is_none());
                assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
                assert_eq!(settlement(&conn, &binding).unwrap(), "published");
                let page = receive(&conn, "sender", None, None, 100).unwrap();
                assert_eq!(page.messages.len(), 1);
                assert_eq!(page.messages[0].interaction_id, first.record.id);
                assert_eq!(page.messages[0].message, "explicit");
                assert_eq!(page.messages[0].reply_status, Some(status.clone()));
                assert_eq!(count(&conn, "structured_replies"), 1);
            }
        }
    }

    #[test]
    fn automatic_reply_preserves_later_explicit_conflict_and_duplicate_semantics() {
        for status in [ReplyStatus::Done, ReplyStatus::Failed, ReplyStatus::Blocked] {
            let conn = database();
            let binding = bind(&conn, &task(&conn, "sender"));
            record_task_turn_outcome(&conn, &binding, status.clone(), "automatic").unwrap();
            let first = publish_task_turn_outcome(&conn, &binding).unwrap().unwrap();
            assert!(!first.duplicate);
            let identical = reply(
                &conn,
                "receiver",
                &binding.request_id,
                status.clone(),
                "automatic",
            )
            .unwrap();
            assert!(identical.duplicate);
            assert_eq!(identical.record.id, first.record.id);
            for later_status in [ReplyStatus::Done, ReplyStatus::Failed, ReplyStatus::Blocked] {
                assert_eq!(
                    reply(
                        &conn,
                        "receiver",
                        &binding.request_id,
                        later_status,
                        "explicit"
                    )
                    .err()
                    .unwrap()
                    .code,
                    "conflicting_reply"
                );
            }
            assert!(publish_task_turn_outcome(&conn, &binding)
                .unwrap()
                .is_none());
            assert_eq!(
                receive(&conn, "sender", None, None, 100)
                    .unwrap()
                    .messages
                    .len(),
                1
            );
        }
    }

    #[test]
    fn shared_exact_turn_fans_out_to_original_requesters() {
        let conn = database();
        let one = task(&conn, "first-requester");
        let two = task(&conn, "second-requester");
        let session = " Session/Case 日本語 ";
        let turn = " Turn:Exact ";
        let one = bind_task_turn(&conn, &one, "codex", session, turn, "start").unwrap();
        let two = bind_task_turn(&conn, &two, "codex", session, turn, "steer").unwrap();
        accepted_input(&conn, &one);
        accepted_input(&conn, &two);
        assert_ne!(one.request_id, two.request_id);
        assert_ne!(one.claim_token, two.claim_token);
        assert_eq!(one.provider_session_id, session);
        assert_eq!(two.provider_turn_id, turn);
        assert_eq!(pending_task_turn_bindings(&conn).unwrap().len(), 2);
        for binding in [&one, &two] {
            record_task_turn_outcome(&conn, binding, ReplyStatus::Done, "shared result").unwrap();
        }
        let replies = recover_task_turn_outcomes(&conn).unwrap();
        assert_eq!(replies.len(), 2);
        for (sender, binding) in [("first-requester", one), ("second-requester", two)] {
            let page = receive(&conn, sender, None, None, 100).unwrap();
            assert_eq!(page.messages.len(), 1);
            assert_eq!(page.messages[0].sender, "receiver");
            assert_eq!(
                page.messages[0].parent_interaction_id.as_deref(),
                Some(binding.request_id.as_str())
            );
            assert_eq!(page.messages[0].message, "shared result");
        }
        assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
    }

    #[test]
    fn host_automation_task_needs_no_requester_and_creates_no_mailbox_recipient() {
        let conn = database();
        let admitted =
            admit_host_automation_task(&conn, "run", "node", "receiver", "work", 7).unwrap();
        let claim = claim_next_task(&conn, "receiver", 7).unwrap().unwrap();
        assert!(claim.record.sender_session_id.is_none());
        let binding = bind(&conn, &claim);
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Blocked, "needs input").unwrap();
        let outcome = publish_task_turn_outcome(&conn, &binding).unwrap().unwrap();
        assert_eq!(outcome.task.id, admitted.record.id);
        assert!(outcome.record.target_session_ids.is_empty());
        assert_eq!(outcome.reply.status, ReplyStatus::Blocked);
        assert_eq!(count(&conn, "agent_message_availability"), 1);
        assert!(publish_task_turn_outcome(&conn, &binding)
            .unwrap()
            .is_none());
    }

    #[test]
    fn known_outbox_survives_reply_failure_reopen_and_new_runtime_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outbox.db");
        let conn = Connection::open(&path).unwrap();
        crate::db::run_migrations(&conn).unwrap();
        let binding = bind(&conn, &task(&conn, "sender"));
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Failed, "terminal output").unwrap();
        conn.execute_batch("CREATE TRIGGER fail_reply BEFORE INSERT ON agent_message_availability WHEN NEW.recipient='sender' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(publish_task_turn_outcome(&conn, &binding).is_err());
        assert_eq!(settlement(&conn, &binding).unwrap(), "outcome_recorded");
        assert_eq!(count(&conn, "structured_replies"), 0);
        assert_eq!(
            load(&conn, &binding.request_id).unwrap().status,
            InteractionStatus::AwaitingReply
        );
        assert!(claim_next_task(&conn, "receiver", 999).unwrap().is_none());
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        crate::db::run_migrations(&conn).unwrap();
        abandon_task_turn_observations(&conn).unwrap();
        assert_eq!(settlement(&conn, &binding).unwrap(), "outcome_recorded");
        conn.execute_batch("DROP TRIGGER fail_reply").unwrap();
        let recovered = recover_task_turn_outcomes(&conn).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].reply.body, "terminal output");
        assert_eq!(recovered[0].reply.status, ReplyStatus::Failed);
        assert_eq!(recovered[0].task.id, binding.request_id);
        let reply_id = recovered[0].record.id.clone();
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
        let page = receive(&conn, "sender", None, None, 100).unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].interaction_id, reply_id);
        assert!(claim_next_task(&conn, "receiver", 999).unwrap().is_none());
    }

    #[test]
    fn published_settlement_failure_rolls_back_reply_but_retains_outbox() {
        let conn = database();
        let binding = bind(&conn, &task(&conn, "sender"));
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "done").unwrap();
        conn.execute_batch("CREATE TRIGGER fail_settlement BEFORE UPDATE ON agent_message_task_turns WHEN NEW.settlement='published' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(publish_task_turn_outcome(&conn, &binding).is_err());
        assert_eq!(count(&conn, "structured_replies"), 0);
        assert_eq!(count(&conn, "agent_message_availability"), 1);
        assert_eq!(settlement(&conn, &binding).unwrap(), "outcome_recorded");
        conn.execute_batch("DROP TRIGGER fail_settlement").unwrap();
        assert_eq!(recover_task_turn_outcomes(&conn).unwrap().len(), 1);
    }

    #[test]
    fn live_recovery_never_reconstructs_bound_unbound_historical_or_uncertain_tasks() {
        let conn = database();
        let bound = bind(&conn, &task(&conn, "sender"));
        let uncertain = bind(&conn, &task(&conn, "sender"));
        mark_task_turn_uncertain(&conn, &uncertain).unwrap();
        mark_task_turn_uncertain(&conn, &uncertain).unwrap();
        let unbound = task(&conn, "sender");
        finish_claim(&conn, &unbound, "provider_accepted").unwrap();
        let historical = task(&conn, "sender");
        conn.execute(
            "DELETE FROM agent_message_delivery WHERE interaction_id=?1",
            [&historical.record.id],
        )
        .unwrap();
        migrate(&conn).unwrap();
        assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
        assert_eq!(settlement(&conn, &bound).unwrap(), "bound");
        assert_eq!(
            pending_task_turn_bindings(&conn).unwrap(),
            vec![bound.clone()]
        );
        assert!(publish_task_turn_outcome(&conn, &bound).unwrap().is_none());
        assert!(
            record_task_turn_outcome(&conn, &uncertain, ReplyStatus::Done, "invented").is_err()
        );
        assert!(publish_task_turn_outcome(&conn, &uncertain)
            .unwrap()
            .is_none());
        abandon_task_turn_observations(&conn).unwrap();
        assert_eq!(settlement(&conn, &bound).unwrap(), "uncertain");
        assert!(pending_task_turn_bindings(&conn).unwrap().is_empty());
        assert!(record_task_turn_outcome(&conn, &bound, ReplyStatus::Done, "invented").is_err());
        assert!(claim_next_task(&conn, "receiver", 8).unwrap().is_none());
        assert_eq!(count(&conn, "structured_replies"), 0);
    }

    #[test]
    fn recorded_outcomes_cannot_be_abandoned_and_first_record_wins() {
        let conn = database();
        let binding = bind(&conn, &task(&conn, "sender"));
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Blocked, "first").unwrap();
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Blocked, "first").unwrap();
        assert_eq!(
            record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "second")
                .unwrap_err()
                .code,
            "conflicting_outcome"
        );
        assert!(mark_task_turn_uncertain(&conn, &binding).is_err());
        abandon_task_turn_observations(&conn).unwrap();
        let first = publish_task_turn_outcome(&conn, &binding).unwrap().unwrap();
        assert_eq!(first.reply.status, ReplyStatus::Blocked);
        assert_eq!(first.reply.body, "first");
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Blocked, "first").unwrap();
        assert!(
            record_task_turn_outcome(&conn, &binding, ReplyStatus::Blocked, "changed").is_err()
        );
    }

    #[test]
    fn terminal_body_is_nonempty_bounded_by_utf8_bytes_and_preserved_exactly() {
        let conn = database();
        let binding = bind(&conn, &task(&conn, "sender"));
        for body in [
            "".into(),
            " \r\n\t".into(),
            "é".repeat(MAX_MESSAGE_BYTES / 2 + 1),
        ] {
            assert_eq!(
                record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, &body)
                    .unwrap_err()
                    .code,
                "invalid_message"
            );
            assert_eq!(settlement(&conn, &binding).unwrap(), "bound");
        }
        let body = "é".repeat(MAX_MESSAGE_BYTES / 2);
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, &body).unwrap();
        assert!(conn
            .execute(
                "UPDATE agent_message_task_turns SET terminal_body=?2 WHERE request_id=?1",
                params![binding.request_id, format!("{body}x")]
            )
            .is_err());
        let replied = publish_task_turn_outcome(&conn, &binding).unwrap().unwrap();
        assert_eq!(replied.reply.body.as_bytes(), body.as_bytes());
        assert_eq!(
            receive(&conn, "sender", None, None, 100).unwrap().messages[0]
                .message
                .as_bytes(),
            body.as_bytes()
        );
    }

    #[test]
    fn deletion_removes_outbox_atomically_and_cannot_resurrect_either_party() {
        for agent in ["sender", "receiver"] {
            let mut conn = database();
            let binding = bind(&conn, &task(&conn, "sender"));
            record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "done").unwrap();
            conn.execute_batch("CREATE TRIGGER fail_delete BEFORE DELETE ON agent_message_delivery BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
            assert!(crate::db::delete_agent_with_conn(&mut conn, agent).is_err());
            assert_eq!(settlement(&conn, &binding).unwrap(), "outcome_recorded");
            conn.execute_batch("DROP TRIGGER fail_delete").unwrap();
            crate::db::delete_agent_with_conn(&mut conn, agent).unwrap();
            assert_eq!(count(&conn, "agent_message_task_turns"), 0);
            assert!(recover_task_turn_outcomes(&conn).unwrap().is_empty());
            assert!(record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "done").is_err());
            assert!(publish_task_turn_outcome(&conn, &binding).is_err());
            assert_eq!(count(&conn, "structured_replies"), 0);
        }
    }

    #[test]
    fn competing_automatic_publications_create_one_reply_and_availability() {
        let conn = database();
        let binding = bind(&conn, &task(&conn, "sender"));
        record_task_turn_outcome(&conn, &binding, ReplyStatus::Done, "done").unwrap();
        let conn = std::sync::Arc::new(std::sync::Mutex::new(conn));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let conn = conn.clone();
                let binding = binding.clone();
                std::thread::spawn(move || {
                    publish_task_turn_outcome(&conn.lock().unwrap(), &binding)
                        .unwrap()
                        .map(|reply| reply.record.id)
                })
            })
            .collect();
        assert_eq!(
            threads
                .into_iter()
                .filter_map(|thread| thread.join().unwrap())
                .count(),
            1
        );
        let conn = conn.lock().unwrap();
        assert_eq!(count(&conn, "structured_replies"), 1);
        assert_eq!(
            receive(&conn, "sender", None, None, 100)
                .unwrap()
                .messages
                .len(),
            1
        );
    }
}
