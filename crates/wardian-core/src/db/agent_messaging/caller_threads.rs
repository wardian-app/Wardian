//! Preserve native caller attribution without granting a child its root's task.
use super::*;
use crate::control::{DeliveryTransportKind, InteractionDeliveryAttemptRecord};

/// Current root snapshot captured under the agent lifecycle gate. These are
/// existing provider identities, not credentials against hostile local clients.
pub struct ProviderThreadCaller<'a> {
    pub provider_thread_id: Option<&'a str>,
    pub current_root_thread_id: Option<&'a str>,
    pub current_generation: u64,
    pub parent_thread_id: Option<&'a str>,
}

/// Informational output stays in the owning agent's conversation. A separate
/// notice preserves attribution even when the original body fills its limit.
pub struct ProviderThreadProgressInput<'a> {
    pub owner: &'a str,
    pub provider_thread_id: &'a str,
    pub parent_thread_id: Option<&'a str>,
    pub body: &'a str,
    pub request_id: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
    pub recipient_origin: Option<&'a RecipientOrigin>,
}

pub struct ProviderThreadProgress {
    pub records: Vec<InteractionRecord>,
    pub duplicate: bool,
}

pub enum ProviderThreadReply {
    Replied(Box<Replied>),
    Progress(ProviderThreadProgress),
}

fn validate_thread_id(thread: &str) -> Result<()> {
    if thread.is_empty()
        || thread.len() > 256
        || thread != thread.trim()
        || thread.chars().any(char::is_control)
    {
        return Err(Error::new(
            "invalid_thread_provenance",
            "Provider thread provenance must be a nonempty, bounded native thread ID.",
        ));
    }
    Ok(())
}

/// Record the live native root on the exact scheduler claim before the input
/// can reach Codex. This authorizes explicit root replies during admission
/// without granting automatic completion before the turn receipt.
pub fn prepare_codex_task_root(conn: &Connection, claim: &TaskClaim, root: &str) -> Result<()> {
    validate_thread_id(root)?;
    if claim.generation == 0 || claim.record.kind != InteractionKind::Task {
        return Err(Error::new(
            "invalid_binding",
            "Expected a current Codex task claim.",
        ));
    }
    let changed = conn.execute(
        "UPDATE agent_message_delivery SET recipient_provider='codex',
         recipient_conversation_id=?4, recipient_generation=?3
         WHERE interaction_id=?1 AND operation='followup_task'
           AND claim_token=?2 AND generation=?3 AND owner='dispatching'
           AND (recipient_conversation_id IS NULL OR
               (recipient_provider='codex' AND recipient_conversation_id=?4 AND recipient_generation=?3))",
        params![claim.record.id, claim.token, claim.generation, root],
    )?;
    if changed != 1 {
        return Err(Error::new(
            "stale_claim",
            "The Codex scheduler claim no longer owns this task and root.",
        ));
    }
    Ok(())
}

fn diagnostic(
    conn: &Connection,
    interaction: &str,
    owner: &str,
    generation: u64,
    thread: Option<&str>,
    state: &str,
    reason: &str,
) -> Result<()> {
    let timestamp = now();
    super::super::upsert_interaction_delivery_attempt_with_conn(
        conn,
        &InteractionDeliveryAttemptRecord {
            id: format!("thread-provenance:{interaction}:{state}"),
            interaction_id: interaction.into(),
            target_session_id: owner.into(),
            transport: DeliveryTransportKind::NativeProvider,
            generation,
            runtime_state: "explicit_messaging".into(),
            delivery_state: state.into(),
            delivery_phase: Some("caller_thread_validation".into()),
            observed_state: thread.map(|id| format!("provider_thread_id={id}")),
            reason: Some(reason.into()),
            error: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        },
    )?;
    Ok(())
}

/// Compare explicit Codex settlement with the assigned native root and current
/// generation in the same transaction as settlement or informational retention.
/// Missing provenance preserves older clients and records that limitation.
pub fn reply_with_provider_thread(
    conn: &Connection,
    sender: &str,
    request_id: &str,
    status: ReplyStatus,
    body: &str,
    caller: ProviderThreadCaller<'_>,
) -> Result<ProviderThreadReply> {
    validate_message(body)?;
    let tx = conn.unchecked_transaction()?;
    let task = load(&tx, request_id)?;
    if task.kind != InteractionKind::Task || task.target_session_ids != [sender] {
        return Err(Error::new(
            "unauthorized",
            "Reply caller is not the task recipient.",
        ));
    }
    let Some(thread) = caller.provider_thread_id else {
        let replied = super::reply_in_transaction(&tx, sender, request_id, status, body, None)?;
        diagnostic(&tx, request_id, sender, caller.current_generation, None, "thread_provenance_unavailable", "Thread provenance unavailable; accepted the explicit reply using the legacy assigned-agent contract.")?;
        tx.commit()?;
        return Ok(ProviderThreadReply::Replied(Box::new(replied)));
    };
    validate_thread_id(thread)?;
    let bound: Option<(String, u64)> = tx.query_row(
        "SELECT provider_session_id,generation FROM agent_message_task_turns WHERE request_id=?1 AND recipient=?2 AND provider='codex'",
        params![request_id, sender], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let assigned = match bound {
        Some(bound) => Some(bound),
        None => tx
            .query_row(
                "SELECT recipient_conversation_id,recipient_generation FROM agent_message_delivery
             WHERE interaction_id=?1 AND recipient=?2 AND operation='followup_task'
               AND owner='dispatching' AND claim_token IS NOT NULL AND recipient_provider='codex'
               AND recipient_conversation_id IS NOT NULL AND recipient_generation=generation",
                params![request_id, sender],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?,
    };
    let current = assigned.as_ref().filter(|(root, generation)| {
        Some(root.as_str()) == caller.current_root_thread_id
            && *generation == caller.current_generation
            && *generation > 0
    });
    if current.is_some_and(|(root, _)| root == thread) {
        let replied = super::reply_in_transaction(&tx, sender, request_id, status, body, None)?;
        tx.commit()?;
        return Ok(ProviderThreadReply::Replied(Box::new(replied)));
    }
    if assigned.is_none() && caller.current_root_thread_id == Some(thread) {
        return Err(Error::new("task_thread_unavailable",
            "This task has no current Codex thread assignment. Wait for Wardian to deliver the task before replying."));
    }
    let origin = current.map(|(root, generation)| RecipientOrigin {
        provider: "codex".into(),
        conversation_id: root.clone(),
        generation: *generation,
    });
    let progress = progress_in_transaction(
        &tx,
        ProviderThreadProgressInput {
            owner: sender,
            provider_thread_id: thread,
            parent_thread_id: caller.parent_thread_id,
            body,
            request_id: Some(request_id),
            idempotency_key: None,
            recipient_origin: origin.as_ref(),
        },
    )?;
    diagnostic(&tx, request_id, sender, caller.current_generation, Some(thread), "foreign_provider_thread", "Reply from a forked, foreign or stale-generation thread cannot settle this task; its body was retained as progress for the owning agent.")?;
    tx.commit()?;
    Ok(ProviderThreadReply::Progress(progress))
}

/// Retain a native child's progress through existing conversation-bound
/// informational delivery. No requester reply or task settlement is created.
pub fn publish_provider_thread_progress(
    conn: &Connection,
    input: ProviderThreadProgressInput<'_>,
) -> Result<ProviderThreadProgress> {
    let tx = conn.unchecked_transaction()?;
    let progress = progress_in_transaction(&tx, input)?;
    tx.commit()?;
    Ok(progress)
}

fn progress_in_transaction(
    conn: &Connection,
    input: ProviderThreadProgressInput<'_>,
) -> Result<ProviderThreadProgress> {
    validate_message(input.body)?;
    validate_thread_id(input.provider_thread_id)?;
    if input
        .idempotency_key
        .is_some_and(|key| key.is_empty() || key.len() > 256)
    {
        return Err(Error::new(
            "invalid_idempotency_key",
            "Key must contain 1 to 256 bytes.",
        ));
    }
    let fingerprint = format!("{:x}", Sha256::digest(input.body.as_bytes()));
    let stored_key = input.idempotency_key.map(|key| {
        format!(
            "native-progress:{:x}",
            Sha256::digest(serde_json::to_vec(&(input.provider_thread_id, key)).unwrap())
        )
    });
    if let Some(key) = stored_key.as_deref() {
        let previous: Option<String> = conn.query_row(
            "SELECT fingerprint FROM agent_message_delivery WHERE sender=?1 AND operation='send_message' AND idempotency_key=?2",
            params![input.owner, key], |row| row.get(0),
        ).optional()?;
        if previous.as_ref().is_some_and(|old| old != &fingerprint) {
            return Err(Error::new(
                "idempotency_conflict",
                "Native progress idempotency key was already used with different content.",
            ));
        }
    }
    let unique = input
        .idempotency_key
        .map(str::to_owned)
        .or_else(|| input.request_id.map(|id| format!("reply:{id}")))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let digest = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(input.owner, input.provider_thread_id, unique, input.body))
                .unwrap()
        )
    );
    let ids = [
        format!("int_thread_notice_{digest}"),
        format!("int_thread_progress_{digest}"),
    ];
    let existing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM interactions WHERE id IN (?1,?2)",
        params![ids[0], ids[1]],
        |row| row.get(0),
    )?;
    if existing == 2 {
        return Ok(ProviderThreadProgress {
            records: vec![load(conn, &ids[0])?, load(conn, &ids[1])?],
            duplicate: true,
        });
    }
    if existing != 0 {
        return Err(Error::new(
            "incomplete_thread_progress",
            "Native thread progress is incomplete; inspect the existing records before retrying.",
        ));
    }
    let attribution = match input.parent_thread_id {
        Some(parent) => format!(
            "Progress from Codex child thread {} (parent thread {parent}).",
            input.provider_thread_id
        ),
        None => format!(
            "Progress from foreign Codex thread {}.",
            input.provider_thread_id
        ),
    };
    let notice = format!("{attribution} The following output is informational for the owning agent and does not settle its canonical task.");
    let timestamp = now();
    let mut records = Vec::new();
    for (index, (id, body)) in ids
        .into_iter()
        .zip([notice.as_str(), input.body])
        .enumerate()
    {
        let record = InteractionRecord {
            id,
            kind: InteractionKind::Message,
            sender_session_id: Some(input.owner.into()),
            target_session_ids: vec![input.owner.into()],
            status: InteractionStatus::Queued,
            trigger_policy: InteractionTriggerPolicy::NotifyOnly,
            body_ref: InteractionBodyRef::Inline { body: body.into() },
            parent_interaction_id: input.request_id.map(str::to_owned),
            created_at: timestamp.clone(),
            updated_at: timestamp.clone(),
            completed_at: None,
        };
        super::super::upsert_interaction_record_with_conn(conn, &record)?;
        // Without verified lineage to a current root, retain the canonical
        // records for inspection, outside both provider information queues.
        let owner = if input.recipient_origin.is_some() {
            "stored"
        } else {
            "blocked_stale_origin"
        };
        conn.execute("INSERT INTO agent_message_delivery(interaction_id,sender,recipient,operation,fingerprint,owner,generation,wake_on_admission,recipient_provider,recipient_conversation_id,recipient_generation,idempotency_key)
            VALUES(?1,?2,?2,'send_message',?7,?9,0,?3,?4,?5,?6,?8)", params![
                record.id, input.owner, input.recipient_origin.map(|_| 1_i64),
                input.recipient_origin.map(|origin| origin.provider.as_str()),
                input.recipient_origin.map(|origin| origin.conversation_id.as_str()),
                input.recipient_origin.map(|origin| origin.generation),
                fingerprint, (index == 1).then_some(stored_key.as_deref()).flatten(), owner,
            ])?;
        make_available(conn, input.owner, &record.id)?;
        diagnostic(
            conn,
            &record.id,
            input.owner,
            input.recipient_origin.map_or(0, |origin| origin.generation),
            Some(input.provider_thread_id),
            "native_child_progress",
            &attribution,
        )?;
        records.push(record);
    }
    Ok(ProviderThreadProgress {
        records,
        duplicate: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn
    }

    fn progress<'a>(body: &'a str, key: Option<&'a str>) -> ProviderThreadProgressInput<'a> {
        ProviderThreadProgressInput {
            owner: "owner",
            provider_thread_id: "child",
            parent_thread_id: Some("root"),
            body,
            request_id: None,
            idempotency_key: key,
            recipient_origin: None,
        }
    }

    #[test]
    fn progress_retry_is_idempotent_and_changed_content_conflicts() {
        let conn = database();
        let first =
            publish_provider_thread_progress(&conn, progress("exact", Some("call"))).unwrap();
        let second =
            publish_provider_thread_progress(&conn, progress("exact", Some("call"))).unwrap();
        assert!(!first.duplicate);
        assert!(second.duplicate);
        assert_eq!(first.records, second.records);
        let changed = publish_provider_thread_progress(&conn, progress("changed", Some("call")));
        assert_eq!(changed.err().unwrap().code, "idempotency_conflict");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM interactions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn independent_unkeyed_progress_keeps_distinct_admissions() {
        let conn = database();
        let first = publish_provider_thread_progress(&conn, progress("same", None)).unwrap();
        let second = publish_provider_thread_progress(&conn, progress("same", None)).unwrap();
        assert_ne!(first.records[1].id, second.records[1].id);
    }

    #[test]
    fn a_body_write_failure_rolls_back_the_notice_and_availability() {
        let conn = database();
        conn.execute_batch("CREATE TRIGGER fail_thread_body BEFORE INSERT ON interactions
            WHEN NEW.id LIKE 'int_thread_progress_%' BEGIN SELECT RAISE(ABORT,'owned progress failure'); END;").unwrap();
        assert!(publish_provider_thread_progress(&conn, progress("body", Some("call"))).is_err());
        for table in [
            "interactions",
            "agent_message_delivery",
            "agent_message_availability",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                count, 0,
                "failed publication must not leave partial {table}"
            );
        }
    }

    #[test]
    fn malformed_present_identity_is_not_treated_as_missing() {
        let conn = database();
        for thread in ["", " child", "child\n"] {
            let mut input = progress("body", None);
            input.provider_thread_id = thread;
            assert_eq!(
                publish_provider_thread_progress(&conn, input)
                    .err()
                    .unwrap()
                    .code,
                "invalid_thread_provenance"
            );
        }
    }
}
