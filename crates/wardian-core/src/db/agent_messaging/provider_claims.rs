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
    let mut statement = conn.prepare("SELECT d.interaction_id FROM agent_message_delivery d JOIN agent_message_availability a ON a.interaction_id=d.interaction_id WHERE d.recipient=?1 AND d.owner='stored' AND d.operation IN ('send_message','reply') AND a.sequence<=?2 ORDER BY a.sequence LIMIT 100")?;
    let rows = statement.query_map(params![recipient, highwater], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
    let changed = tx.execute("UPDATE agent_message_delivery SET owner='dispatching',generation=?3,claim_token=?4 WHERE interaction_id=?1 AND recipient=?2 AND owner='stored' AND operation IN ('send_message','reply')", params![id, recipient, generation, token])?;
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
    let changed = conn.execute("UPDATE agent_message_delivery SET owner=CASE WHEN operation='followup_task' THEN 'pending' ELSE 'stored' END,claim_token=NULL WHERE interaction_id=?1 AND claim_token=?2 AND generation=?3 AND owner='dispatching'", params![claim.record.id, claim.token, claim.generation])?;
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
    })
}
