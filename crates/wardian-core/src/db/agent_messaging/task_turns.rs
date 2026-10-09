//! Exact scheduler admissions and a durable terminal outbox. Never replay provider work.
use super::*;

mod final_results;
pub use final_results::{record_task_turn_final, RecordedTaskFinal};

/// Maximum number of admitted task messages restored after a Codex compaction.
/// The query reads one extra row so callers can fail closed on overflow.
pub const MAX_COMPACTED_TASK_CONTEXTS: usize = 16;

pub use crate::agent_messaging::RecoveredTaskContext as CompactedTaskContext;

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
        ON agent_message_task_turns(settlement);
        CREATE TABLE IF NOT EXISTS agent_message_task_turn_observations (
        request_id TEXT PRIMARY KEY NOT NULL REFERENCES agent_message_task_turns(request_id) ON DELETE CASCADE,
        provider_status TEXT NOT NULL CHECK(provider_status IN ('completed','interrupted','failed')),
        answer_sha256 TEXT NOT NULL CHECK(length(answer_sha256)=64),
        answer_bytes INTEGER NOT NULL CHECK(answer_bytes>=0),
        answer_preview TEXT NOT NULL CHECK(length(CAST(answer_preview AS BLOB))<={MAX_MESSAGE_BYTES}),
        diagnostic TEXT NOT NULL CHECK(length(CAST(diagnostic AS BLOB))<=1024),
        information_id TEXT UNIQUE,
        observed_at TEXT NOT NULL);"
    ))
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

fn record_outcome_in_transaction(
    tx: &Connection,
    binding: &TaskTurnBinding,
    status: ReplyStatus,
    body: &str,
) -> Result<()> {
    let state = settlement(tx, binding)?;
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
        Some(reply_in_transaction(
            &tx,
            &binding.recipient,
            &binding.request_id,
            super::super::enum_from_value(&status)?,
            &body,
            None,
        )?)
    };
    tx.execute(
        "UPDATE agent_message_task_turns SET settlement='published' WHERE request_id=?1",
        [&binding.request_id],
    )?;
    tx.commit()?;
    Ok(replied)
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
    let mut statement = conn.prepare(
        "SELECT request_id,claim_token,recipient,generation,provider,provider_session_id,provider_turn_id,admission_mode
         FROM agent_message_task_turns b WHERE settlement='bound'
         AND NOT EXISTS(SELECT 1 FROM agent_message_task_turn_observations o WHERE o.request_id=b.request_id)
         ORDER BY request_id",
    )?;
    let rows = statement.query_map([], row_binding)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Return only unresolved, provider-accepted tasks bound to this exact Codex turn.
/// Availability order describes inbox chronology only. The extra row lets the
/// caller reject an oversized projection without sending a partial task list.
pub fn compacted_task_contexts(
    conn: &Connection,
    recipient: &str,
    generation: u64,
    provider_session_id: &str,
    provider_turn_id: &str,
) -> Result<Vec<CompactedTaskContext>> {
    let mut statement = conn.prepare(
        "SELECT a.sequence,b.request_id FROM agent_message_availability a
        JOIN agent_message_task_turns b ON b.request_id=a.interaction_id
        JOIN agent_message_delivery d ON d.interaction_id=b.request_id
        JOIN interactions i ON i.id=b.request_id
        WHERE a.recipient=?1 AND b.recipient=?1 AND b.generation=?2
          AND b.provider='codex' AND b.provider_session_id=?3 AND b.provider_turn_id=?4
          AND b.settlement='bound' AND d.owner='provider_accepted'
          AND d.generation=b.generation AND d.claim_token=b.claim_token
          AND d.recipient=b.recipient AND d.operation='followup_task'
          AND i.kind='task' AND i.status='awaiting_reply'
          AND NOT EXISTS(SELECT 1 FROM structured_replies r WHERE r.request_id=b.request_id)
          AND NOT EXISTS(SELECT 1 FROM agent_message_task_turn_observations o WHERE o.request_id=b.request_id)
        ORDER BY a.sequence,b.request_id LIMIT ?5",
    )?;
    let rows = statement.query_map(
        params![
            recipient,
            generation,
            provider_session_id,
            provider_turn_id,
            MAX_COMPACTED_TASK_CONTEXTS as i64 + 1,
        ],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )?;
    let identities = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    identities
        .into_iter()
        .map(|(availability_sequence, request_id)| {
            let record = load(conn, &request_id)?;
            if record.kind != InteractionKind::Task
                || record.status != InteractionStatus::AwaitingReply
                || record.target_session_ids != [recipient.to_owned()]
            {
                return Err(Error::new(
                    "invalid_task",
                    "Bound task no longer matches its canonical recipient or status.",
                ));
            }
            let context = message_context(conn, &record)?;
            Ok(CompactedTaskContext {
                availability_sequence,
                created_at: record.created_at,
                message: context,
            })
        })
        .collect()
}

/// Process-start hydrate retires only observations without a recorded outcome.
/// Live receive recovery must not retire a turn that the runtime still observes.
pub fn abandon_task_turn_observations(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE agent_message_task_turns SET settlement='uncertain' WHERE settlement='bound' AND terminal_status IS NULL AND terminal_body IS NULL
        AND NOT EXISTS(SELECT 1 FROM agent_message_task_turn_observations o WHERE o.request_id=agent_message_task_turns.request_id)", [])?;
    Ok(())
}

/// Retire an admission whose exact provider continuity was lost, without rerunning
/// or reconstructing it. Known outcomes must instead remain recoverable.
pub fn mark_task_turn_uncertain(conn: &Connection, binding: &TaskTurnBinding) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    match settlement(&tx, binding)?.as_str() {
        "bound" => {
            let finished: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_message_task_turn_observations WHERE request_id=?1)",
                [&binding.request_id], |row| row.get(0),
            )?;
            if finished {
                return Err(Error::new(
                    "invalid_state",
                    "Known finished turns cannot be discarded as uncertain.",
                ));
            }
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

    /// Seed a single outcome for terminal-outbox recovery fixtures.
    /// Production validates and records the whole final packet through `record_task_turn_final`.
    fn record_task_turn_outcome(
        conn: &Connection,
        binding: &TaskTurnBinding,
        status: ReplyStatus,
        body: &str,
    ) -> Result<()> {
        validate_message(body)?;
        let tx = conn.unchecked_transaction()?;
        record_outcome_in_transaction(&tx, binding, status, body)?;
        tx.commit()?;
        Ok(())
    }

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
        bind_task_turn(conn, claim, "codex", "session", "turn", "start").unwrap()
    }

    fn task_for(
        conn: &Connection,
        sender: &str,
        recipient: &str,
        message: &str,
        generation: u64,
    ) -> TaskClaim {
        admit(
            conn,
            Admission {
                sender,
                recipient,
                message,
                idempotency_key: None,
                task: true,
                generation,
            },
        )
        .unwrap();
        claim_next_task(conn, recipient, generation)
            .unwrap()
            .unwrap()
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
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

    #[test]
    fn compacted_context_is_exact_ordered_and_excludes_settled_or_unbound_tasks() {
        let conn = database();
        let first = task_for(&conn, "requester-a", "receiver", "first task", 7);
        bind_task_turn(&conn, &first, "codex", "session", "turn", "steer").unwrap();
        let second = task_for(&conn, "requester-b", "receiver", "second task", 7);
        bind_task_turn(&conn, &second, "codex", "session", "turn", "steer").unwrap();

        let foreign_session = task_for(&conn, "requester", "receiver", "other session", 7);
        bind_task_turn(
            &conn,
            &foreign_session,
            "codex",
            "other-session",
            "turn",
            "steer",
        )
        .unwrap();
        let foreign_turn = task_for(&conn, "requester", "receiver", "other turn", 7);
        bind_task_turn(
            &conn,
            &foreign_turn,
            "codex",
            "session",
            "other-turn",
            "steer",
        )
        .unwrap();
        let foreign_generation = task_for(&conn, "requester", "receiver", "other generation", 8);
        bind_task_turn(
            &conn,
            &foreign_generation,
            "codex",
            "session",
            "turn",
            "steer",
        )
        .unwrap();
        let foreign_recipient = task_for(&conn, "requester", "other-agent", "other recipient", 7);
        bind_task_turn(
            &conn,
            &foreign_recipient,
            "codex",
            "session",
            "turn",
            "steer",
        )
        .unwrap();

        let completed = task_for(&conn, "requester", "receiver", "already replied", 7);
        let completed_binding =
            bind_task_turn(&conn, &completed, "codex", "session", "turn", "steer").unwrap();
        reply(
            &conn,
            "receiver",
            &completed_binding.request_id,
            ReplyStatus::Done,
            "explicit reply",
        )
        .unwrap();
        let uncertain = task_for(&conn, "requester", "receiver", "uncertain", 7);
        let uncertain_binding =
            bind_task_turn(&conn, &uncertain, "codex", "session", "turn", "steer").unwrap();
        mark_task_turn_uncertain(&conn, &uncertain_binding).unwrap();
        admit(
            &conn,
            Admission {
                sender: "requester",
                recipient: "receiver",
                message: "not yet admitted to a turn",
                idempotency_key: None,
                task: true,
                generation: 7,
            },
        )
        .unwrap();

        let contexts = compacted_task_contexts(&conn, "receiver", 7, "session", "turn").unwrap();
        assert_eq!(contexts.len(), 2);
        assert!(contexts[0].availability_sequence < contexts[1].availability_sequence);
        assert_eq!(contexts[0].message.sender, "requester-a");
        assert_eq!(contexts[0].message.body, "first task");
        assert_eq!(
            contexts[0].message.request_id.as_deref(),
            Some(first.record.id.as_str())
        );
        assert_eq!(contexts[1].message.sender, "requester-b");
        assert_eq!(contexts[1].message.body, "second task");
        assert_eq!(
            contexts[1].message.request_id.as_deref(),
            Some(second.record.id.as_str())
        );
        assert!(contexts.iter().all(|context| {
            context.message.recipient == "receiver"
                && context.message.kind == InteractionKind::Task
                && !context.created_at.is_empty()
        }));
    }

    #[test]
    fn compaction_projection_stays_empty_until_durable_turn_binding_commits() {
        let conn = database();
        let claim = task_for(&conn, "requester", "receiver", "admitted task", 7);
        assert!(
            compacted_task_contexts(&conn, "receiver", 7, "session", "turn")
                .unwrap()
                .is_empty()
        );

        bind_task_turn(&conn, &claim, "codex", "session", "turn", "start").unwrap();
        let contexts = compacted_task_contexts(&conn, "receiver", 7, "session", "turn").unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(
            contexts[0].message.request_id.as_deref(),
            Some(claim.record.id.as_str())
        );
        assert_eq!(contexts[0].message.body, "admitted task");
    }

    #[test]
    fn task_context_read_is_non_consuming_and_count_overflow_is_not_partial() {
        let conn = database();
        for index in 0..=MAX_COMPACTED_TASK_CONTEXTS {
            let claim = task_for(&conn, "requester", "receiver", &format!("task {index}"), 7);
            bind_task_turn(&conn, &claim, "codex", "thread", "turn", "steer").unwrap();
        }
        let before: i64 = conn
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        let first = compacted_task_contexts(&conn, "receiver", 7, "thread", "turn").unwrap();
        let second = compacted_task_contexts(&conn, "receiver", 7, "thread", "turn").unwrap();
        let after: i64 = conn
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(first.len(), MAX_COMPACTED_TASK_CONTEXTS + 1);
        assert_eq!(first, second);
        assert_eq!(before, after);
    }
}
