use super::*;
use crate::agent_messaging::AgentMessageContext;

/// Verify the exact unreconciled provider claim without changing ownership.
pub fn owns_claim(conn: &Connection, claim: &TaskClaim) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_message_delivery WHERE interaction_id=?1 AND claim_token=?2 AND generation=?3 AND owner='dispatching')", params![claim.record.id, claim.token, claim.generation], |row| row.get(0))?)
}

/// Queue progress marker; body access and ownership still occur at claim time.
pub fn next_pending_task_id(conn: &Connection, recipient: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT d.interaction_id FROM agent_message_delivery d JOIN agent_message_availability a ON a.interaction_id=d.interaction_id JOIN interactions i ON i.id=d.interaction_id WHERE d.recipient=?1 AND d.owner='pending' AND d.operation='followup_task' AND i.status='awaiting_reply' ORDER BY a.sequence LIMIT 1", [recipient], |row| row.get(0)).optional()?)
}

/// Snapshot the finite information set to drain when an explicit task starts.
/// Later arrivals have their own push opportunity and cannot starve that task.
pub fn information_highwater(conn: &Connection, recipient: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(sequence),0) FROM agent_message_availability WHERE recipient=?1",
        [recipient],
        |row| row.get(0),
    )?)
}

/// Return at most 100 references, never a second payload collection.
pub fn pending_information(
    conn: &Connection,
    recipient: &str,
    highwater: i64,
) -> Result<Vec<String>> {
    let mut statement = conn.prepare("SELECT d.interaction_id FROM agent_message_delivery d JOIN agent_message_availability a ON a.interaction_id=d.interaction_id WHERE d.recipient=?1 AND d.owner='stored' AND d.operation='send_message' AND COALESCE(d.wake_on_admission,0)=0 AND a.sequence<=?2 ORDER BY a.sequence LIMIT 100")?;
    let rows = statement.query_map(params![recipient, highwater], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Return only newly admitted managed messages that carry server-owned wake
/// eligibility. Legacy rows remain outside this scan after schema migration.
pub fn pending_wakeable_information(conn: &Connection, recipient: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT d.interaction_id FROM agent_message_delivery d
         JOIN agent_message_availability a ON a.interaction_id=d.interaction_id
         WHERE d.recipient=?1 AND d.owner='stored' AND d.operation='send_message'
           AND d.wake_on_admission=1
         ORDER BY a.sequence LIMIT 100",
    )?;
    let rows = statement.query_map([recipient], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A new-session or provider-change fence is terminal for this message. The
/// canonical row remains available to read-only diagnostics and is never sent
/// into a different conversation.
pub fn stale_wakeable_information(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE agent_message_delivery SET owner='blocked_stale_origin'
         WHERE interaction_id=?1 AND operation='send_message'
           AND wake_on_admission=1 AND owner='stored'",
        [id],
    )? == 1)
}

/// Return bounded canonical replies awaiting an automatic continuation.
pub fn pending_reply_continuations(conn: &Connection, recipient: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT d.interaction_id FROM agent_message_delivery d
         JOIN agent_message_availability a ON a.interaction_id=d.interaction_id
         WHERE d.recipient=?1 AND d.owner='stored' AND d.operation='reply'
           AND d.continuation_state IN ('pending','waiting_busy','waiting_paused','waiting_identity')
         ORDER BY a.sequence LIMIT 100",
    )?;
    let rows = statement.query_map([recipient], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Explicit wakeups may name a just-committed reply, but must never replay a
/// delivered, uncertain, blocked, or currently claimed continuation.
pub fn reply_continuation_is_pending(conn: &Connection, recipient: &str, id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_message_delivery
         WHERE interaction_id=?1 AND recipient=?2 AND operation='reply'
           AND owner='stored'
           AND continuation_state IN ('pending','waiting_busy','waiting_paused','waiting_identity'))",
        params![id, recipient],
        |row| row.get(0),
    )?)
}

/// Claim one correlated reply for provider continuation, separate from FYI pushes.
pub fn claim_reply_continuation(
    conn: &Connection,
    recipient: &str,
    id: &str,
    generation: u64,
) -> Result<Option<TaskClaim>> {
    let tx = conn.unchecked_transaction()?;
    let token = new_id("claim");
    let changed = tx.execute(
        "UPDATE agent_message_delivery
         SET owner='dispatching',generation=?3,claim_token=?4
         WHERE interaction_id=?1 AND recipient=?2 AND owner='stored'
           AND operation='reply' AND continuation_state='pending'
           AND EXISTS (SELECT 1 FROM interactions i
                       WHERE i.id=agent_message_delivery.interaction_id AND i.kind='reply')",
        params![id, recipient, generation, token],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    let record = load(&tx, id)?;
    tx.commit()?;
    Ok(Some(TaskClaim {
        record,
        token,
        generation,
    }))
}

/// Record a durable reply continuation disposition without creating another queue.
pub fn set_reply_continuation_state(conn: &Connection, id: &str, state: &str) -> Result<bool> {
    if !matches!(
        state,
        "pending"
            | "waiting_busy"
            | "waiting_paused"
            | "waiting_identity"
            | "blocked_legacy_uncertain"
            | "blocked_stale_origin"
    ) {
        return Err(Error::new(
            "invalid_state",
            "Invalid reply continuation state.",
        ));
    }
    let owner = if state.starts_with("blocked_") {
        state
    } else {
        "stored"
    };
    let changed = conn.execute(
        "UPDATE agent_message_delivery SET continuation_state=?2,owner=?3
         WHERE interaction_id=?1 AND operation='reply' AND owner='stored'",
        params![id, state, owner],
    )?;
    Ok(changed == 1)
}

/// Information and replies compete with receive at the same atomic boundary.
/// Tasks are excluded: only the explicit followup path can admit work.
pub fn claim_information(
    conn: &Connection,
    recipient: &str,
    id: &str,
    generation: u64,
) -> Result<Option<TaskClaim>> {
    let tx = conn.unchecked_transaction()?;
    let token = new_id("claim");
    let changed = tx.execute("UPDATE agent_message_delivery SET owner='dispatching',generation=?3,claim_token=?4 WHERE interaction_id=?1 AND recipient=?2 AND owner='stored' AND operation='send_message'", params![id, recipient, generation, token])?;
    if changed == 0 {
        return Ok(None);
    }
    let record = load(&tx, id)?;
    tx.commit()?;
    Ok(Some(TaskClaim {
        record,
        token,
        generation,
    }))
}

/// Claim only an explicitly wakeable new message. Historical stored FYIs cannot
/// be promoted by dispatch or startup recovery.
pub fn claim_wakeable_information(
    conn: &Connection,
    recipient: &str,
    id: &str,
    generation: u64,
) -> Result<Option<TaskClaim>> {
    let tx = conn.unchecked_transaction()?;
    let token = new_id("claim");
    let changed = tx.execute(
        "UPDATE agent_message_delivery SET owner='dispatching',generation=?3,claim_token=?4
         WHERE interaction_id=?1 AND recipient=?2 AND owner='stored'
           AND operation='send_message' AND wake_on_admission=1",
        params![id, recipient, generation, token],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    let record = load(&tx, id)?;
    tx.commit()?;
    Ok(Some(TaskClaim {
        record,
        token,
        generation,
    }))
}

/// Caller must have explicit transport proof that no provider write occurred.
/// A crossed or uncertain boundary never permits this transition.
pub fn release_before_write(conn: &Connection, claim: &TaskClaim) -> Result<()> {
    let changed = conn.execute("UPDATE agent_message_delivery SET owner=CASE WHEN operation='followup_task' THEN 'pending' ELSE 'stored' END,claim_token=NULL,continuation_state=CASE WHEN operation='reply' THEN 'pending' ELSE continuation_state END WHERE interaction_id=?1 AND claim_token=?2 AND generation=?3 AND owner='dispatching'", params![claim.record.id, claim.token, claim.generation])?;
    if changed != 1 {
        return Err(Error::new(
            "stale_claim",
            "Delivery claim no longer owns this message.",
        ));
    }
    Ok(())
}

/// Build context only from the canonical interaction and its reply projection.
pub fn message_context(
    conn: &Connection,
    record: &InteractionRecord,
) -> Result<AgentMessageContext> {
    let InteractionBodyRef::Inline { body } = &record.body_ref else {
        return Err(Error::new(
            "invalid_message",
            "Canonical message body is not inline.",
        ));
    };
    let recipient = record
        .target_session_ids
        .first()
        .cloned()
        .ok_or_else(|| Error::new("invalid_message", "Canonical message has no recipient."))?;
    let reply_status = if record.kind == InteractionKind::Reply {
        let status: String = conn.query_row(
            "SELECT status FROM structured_replies WHERE request_id=?1",
            [record.parent_interaction_id.as_deref().unwrap_or("")],
            |row| row.get(0),
        )?;
        Some(super::super::enum_from_value(&status)?)
    } else {
        None
    };
    Ok(AgentMessageContext {
        schema_version: 1,
        sender: record.sender_session_id.clone().unwrap_or_default(),
        host_automation: super::host_automation_provenance(conn, &record.id)?,
        recipient,
        kind: record.kind,
        interaction_id: record.id.clone(),
        parent_interaction_id: record.parent_interaction_id.clone(),
        request_id: match record.kind {
            InteractionKind::Task => Some(record.id.clone()),
            InteractionKind::Reply => record.parent_interaction_id.clone(),
            _ => None,
        },
        body: body.clone(),
        reply_status,
        pending_information: Vec::new(),
    })
}
