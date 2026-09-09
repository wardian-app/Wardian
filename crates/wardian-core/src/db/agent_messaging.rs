//! V2 delivery metadata references canonical interactions; no payload is stored here.
use crate::agent_messaging::{
    AgentMessage, AgentMessagePage, AgentMessagingError as Error, TaskDeliveryOwner,
    MAX_MESSAGE_BYTES,
};
use crate::control::{
    InteractionBodyRef, InteractionKind, InteractionRecord, InteractionStatus,
    InteractionTriggerPolicy, ReplyStatus, StructuredReply,
};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Error>;
mod provider_claims;
pub use provider_claims::{
    claim_information, information_highwater, message_context, next_pending_task_id, owns_claim,
    pending_information, release_before_write,
};

/// Availability is append-only and never backfilled from legacy interactions.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_message_delivery (
        interaction_id TEXT PRIMARY KEY, sender TEXT NOT NULL, recipient TEXT NOT NULL,
        operation TEXT NOT NULL, idempotency_key TEXT, fingerprint TEXT NOT NULL,
        owner TEXT NOT NULL, generation INTEGER NOT NULL, claim_token TEXT,
        UNIQUE(sender, operation, idempotency_key));
        CREATE TABLE IF NOT EXISTS agent_message_availability (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT, recipient TEXT NOT NULL,
        interaction_id TEXT NOT NULL, UNIQUE(recipient, interaction_id));
        CREATE INDEX IF NOT EXISTS agent_message_recipient_sequence
        ON agent_message_availability(recipient, sequence);
        CREATE TABLE IF NOT EXISTS agent_message_cursors (
        token TEXT PRIMARY KEY, recipient TEXT NOT NULL, sequence INTEGER NOT NULL,
        UNIQUE(recipient, sequence));
        CREATE TABLE IF NOT EXISTS agent_message_ack (
        recipient TEXT PRIMARY KEY, sequence INTEGER NOT NULL DEFAULT 0);",
    )
}

/// Run against Wardian's existing serialized database connection.
pub fn with_db<T>(f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    super::get_db_conn(|conn| Ok(f(conn)))
        .map_err(|error| Error::new("storage_error", error.to_string()))?
}

/// Admission is unclaimed; receive and scheduler compete at delivery time.
pub struct Admission<'a> {
    pub sender: &'a str,
    pub recipient: &'a str,
    pub message: &'a str,
    pub idempotency_key: Option<&'a str>,
    pub task: bool,
    pub generation: u64,
}

pub struct Admitted {
    pub record: InteractionRecord,
    pub owner: TaskDeliveryOwner,
    pub duplicate: bool,
    pub delivery_state: String,
}

pub fn validate_message(message: &str) -> Result<()> {
    if message.trim().is_empty() || message.len() > MAX_MESSAGE_BYTES {
        return Err(Error::new(
            "invalid_message",
            "Message must contain text and be at most 64 KiB.",
        ));
    }
    Ok(())
}

pub fn admit(conn: &Connection, request: Admission<'_>) -> Result<Admitted> {
    validate_message(request.message)?;
    if request
        .idempotency_key
        .is_some_and(|key| key.is_empty() || key.len() > 256)
    {
        return Err(Error::new(
            "invalid_idempotency_key",
            "Key must contain 1 to 256 bytes.",
        ));
    }
    let operation = if request.task {
        "followup_task"
    } else {
        "send_message"
    };
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(request.recipient, request.message)).unwrap())
    );
    let tx = conn.unchecked_transaction()?;
    if let Some(key) = request.idempotency_key {
        let existing: Option<(String, String, String)> = tx.query_row(
            "SELECT interaction_id, fingerprint, owner FROM agent_message_delivery WHERE sender=?1 AND operation=?2 AND idempotency_key=?3",
            params![request.sender, operation, key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
        if let Some((id, previous, owner)) = existing {
            if previous != fingerprint {
                return Err(Error::new(
                    "idempotency_conflict",
                    "Key already admitted a different target or body.",
                ));
            }
            let record = load(&tx, &id)?;
            return Ok(Admitted {
                record,
                owner: delivery_owner(&owner),
                delivery_state: owner,
                duplicate: true,
            });
        }
    }
    let owner = if request.task { "pending" } else { "stored" };
    let now = now();
    let record = InteractionRecord {
        id: new_id(if request.task { "ask" } else { "int" }),
        kind: if request.task {
            InteractionKind::Task
        } else {
            InteractionKind::Message
        },
        sender_session_id: Some(request.sender.into()),
        target_session_ids: vec![request.recipient.into()],
        status: if request.task {
            InteractionStatus::AwaitingReply
        } else {
            InteractionStatus::Queued
        },
        trigger_policy: if request.task {
            InteractionTriggerPolicy::ReplyRequired
        } else {
            InteractionTriggerPolicy::NotifyOnly
        },
        body_ref: InteractionBodyRef::Inline {
            body: request.message.into(),
        },
        parent_interaction_id: None,
        created_at: now.clone(),
        updated_at: now,
        completed_at: None,
    };
    super::upsert_interaction_record_with_conn(&tx, &record)?;
    tx.execute("INSERT INTO agent_message_delivery(interaction_id,sender,recipient,operation,idempotency_key,fingerprint,owner,generation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![record.id,request.sender,request.recipient,operation,request.idempotency_key,fingerprint,owner,request.generation])?;
    make_available(&tx, request.recipient, &record.id)?;
    tx.commit()?;
    Ok(Admitted {
        record,
        owner: delivery_owner(owner),
        delivery_state: owner.into(),
        duplicate: false,
    })
}

fn make_available(conn: &Connection, recipient: &str, id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO agent_message_availability(recipient,interaction_id) VALUES(?1,?2)",
        params![recipient, id],
    )?;
    Ok(())
}

pub fn load(conn: &Connection, id: &str) -> Result<InteractionRecord> {
    conn.query_row("SELECT id,kind,sender_session_id,target_session_ids,status,trigger_policy,body_ref,parent_interaction_id,created_at,updated_at,completed_at FROM interactions WHERE id=?1", [id], super::row_to_interaction_record)
        .optional()?.ok_or_else(|| Error::new("not_found", "Interaction does not exist."))
}

pub struct Replied {
    pub task: InteractionRecord,
    pub record: InteractionRecord,
    pub reply: StructuredReply,
    pub duplicate: bool,
}

/// Canonical task, reply, legacy structured projection, and recipient availability
/// commit together. Repeating the identical authorized reply returns its identity.
pub fn reply(
    conn: &Connection,
    sender: &str,
    request_id: &str,
    status: ReplyStatus,
    message: &str,
) -> Result<Replied> {
    reply_with_claim(conn, sender, request_id, status, message, None)
}

/// A definite startup failure completes the claimed task with a clearly
/// attributed Wardian failure notice. It is not a model-authored reply.
pub fn reply_startup_failure(conn: &Connection, claim: &TaskClaim) -> Result<Replied> {
    let sender = claim
        .record
        .target_session_ids
        .first()
        .ok_or_else(|| Error::new("invalid_task", "Task has no recipient."))?;
    reply_with_claim(conn, sender, &claim.record.id, ReplyStatus::Failed,
        "Wardian delivery failed before this task reached the provider. No work was submitted. Inspect the recipient's delivery diagnostics before assigning a new task.", Some(claim))
}

fn reply_with_claim(
    conn: &Connection,
    sender: &str,
    request_id: &str,
    status: ReplyStatus,
    message: &str,
    claim: Option<&TaskClaim>,
) -> Result<Replied> {
    validate_message(message)?;
    let tx = conn.unchecked_transaction()?;
    if let Some(claim) = claim {
        if !owns_claim(&tx, claim)? {
            return Err(Error::new(
                "stale_claim",
                "Startup failure no longer owns this task.",
            ));
        }
    }
    let mut task = load(&tx, request_id)?;
    if task.kind != InteractionKind::Task || task.target_session_ids != [sender] {
        return Err(Error::new(
            "unauthorized",
            "Reply caller is not the task recipient.",
        ));
    }
    let is_v2: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_message_delivery WHERE interaction_id=?1 AND operation='followup_task')",[request_id],|row|row.get(0))?;
    if !is_v2 {
        return Err(Error::new("not_found", "Task is not a v2 messaging task."));
    }
    let previous: Option<(String, String, String)> = tx
        .query_row(
            "SELECT status,body,replied_at FROM structured_replies WHERE request_id=?1",
            [request_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((old_status, body, replied_at)) = previous {
        if old_status != super::enum_value(&status)? || body != message {
            return Err(Error::new(
                "conflicting_reply",
                "Task already has a different terminal reply.",
            ));
        }
        let id: String = tx.query_row(
            "SELECT id FROM interactions WHERE kind='reply' AND parent_interaction_id=?1",
            [request_id],
            |row| row.get(0),
        )?;
        let record = load(&tx, &id)?;
        let reply = StructuredReply {
            request_id: request_id.into(),
            status,
            body,
            target_session_id: sender.into(),
            source_session_id: Some(sender.into()),
            replied_at,
        };
        return Ok(Replied {
            task,
            record,
            reply,
            duplicate: true,
        });
    }
    if task.status != InteractionStatus::AwaitingReply {
        return Err(Error::new("conflicting_reply", "Task is already terminal."));
    }
    let recipient = task
        .sender_session_id
        .clone()
        .ok_or_else(|| Error::new("invalid_task", "Task has no requester."))?;
    let now = now();
    task.status = InteractionStatus::Completed;
    task.updated_at = now.clone();
    task.completed_at = Some(now.clone());
    let record = InteractionRecord {
        id: new_id("int"),
        kind: InteractionKind::Reply,
        sender_session_id: Some(sender.into()),
        target_session_ids: vec![recipient.clone()],
        status: InteractionStatus::Completed,
        trigger_policy: InteractionTriggerPolicy::NotifyOnly,
        body_ref: InteractionBodyRef::Inline {
            body: message.into(),
        },
        parent_interaction_id: Some(request_id.into()),
        created_at: now.clone(),
        updated_at: now.clone(),
        completed_at: Some(now.clone()),
    };
    let reply = StructuredReply {
        request_id: request_id.into(),
        status,
        body: message.into(),
        target_session_id: sender.into(),
        source_session_id: Some(sender.into()),
        replied_at: now,
    };
    super::upsert_interaction_record_with_conn(&tx, &task)?;
    super::upsert_interaction_record_with_conn(&tx, &record)?;
    super::upsert_structured_reply_with_conn(&tx, &reply)?;
    tx.execute("INSERT INTO agent_message_delivery(interaction_id,sender,recipient,operation,fingerprint,owner,generation) VALUES(?1,?2,?3,'reply','','stored',0)", params![record.id, sender, recipient])?;
    make_available(&tx, &recipient, &record.id)?;
    if let Some(claim) = claim {
        finish_claim(&tx, claim, "failed_before_submit")?;
    }
    tx.commit()?;
    Ok(Replied {
        task,
        record,
        reply,
        duplicate: false,
    })
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn delivery_owner(state: &str) -> TaskDeliveryOwner {
    match state {
        "pending" => TaskDeliveryOwner::Unclaimed,
        "receiver_available" | "stored" => TaskDeliveryOwner::Receiver,
        _ => TaskDeliveryOwner::Scheduler,
    }
}

/// Remove metadata inside the existing canonical agent-deletion transaction.
/// Other recipients keep valid highwater cursors even when their referenced
/// messages are deleted; sequences are never reused or historical rows backfilled.
pub(super) fn delete_references(
    conn: &Connection,
    recipient: &str,
    interaction_ids: &[String],
) -> rusqlite::Result<()> {
    for id in interaction_ids {
        conn.execute(
            "DELETE FROM agent_message_availability WHERE interaction_id=?1",
            [id],
        )?;
        conn.execute(
            "DELETE FROM agent_message_delivery WHERE interaction_id=?1",
            [id],
        )?;
    }
    conn.execute(
        "DELETE FROM agent_message_availability WHERE recipient=?1",
        [recipient],
    )?;
    conn.execute(
        "DELETE FROM agent_message_delivery WHERE recipient=?1 OR sender=?1",
        [recipient],
    )?;
    conn.execute(
        "DELETE FROM agent_message_cursors WHERE recipient=?1",
        [recipient],
    )?;
    conn.execute(
        "DELETE FROM agent_message_ack WHERE recipient=?1",
        [recipient],
    )?;
    Ok(())
}

/// Identify the v2 reply transaction boundary for legacy CLI reply compatibility.
pub fn is_task(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_message_delivery WHERE interaction_id=?1 AND operation='followup_task')", [id], |row| row.get(0))?)
}

fn cursor_sequence(conn: &Connection, recipient: &str, token: &str) -> Result<i64> {
    if !token.starts_with("am1_") || token.len() != 36 {
        return Err(Error::new(
            "invalid_cursor",
            "Invalid cursor version or format.",
        ));
    }
    let cursor: Option<(String, i64)> = conn
        .query_row(
            "SELECT recipient,sequence FROM agent_message_cursors WHERE token=?1",
            [token],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match cursor {
        Some((owner, sequence)) if owner == recipient => Ok(sequence),
        Some(_) => Err(Error::new(
            "invalid_cursor",
            "Cursor belongs to another recipient.",
        )),
        None => Err(Error::new(
            "expired_cursor",
            "Cursor is unknown or no longer available.",
        )),
    }
}

fn issue_cursor(conn: &Connection, recipient: &str, sequence: i64) -> Result<String> {
    let existing = conn
        .query_row(
            "SELECT token FROM agent_message_cursors WHERE recipient=?1 AND sequence=?2",
            params![recipient, sequence],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(token) = existing {
        return Ok(token);
    }
    let token = new_id("am1");
    conn.execute(
        "INSERT INTO agent_message_cursors(token,recipient,sequence) VALUES(?1,?2,?3)",
        params![token, recipient, sequence],
    )?;
    Ok(token)
}

/// Read/claim a bounded page transactionally. Replaying its input cursor returns
/// the same identities; a receiver claim never dispatches work. Only ack_cursor
/// advances acknowledgement, bounded by a cursor this recipient was issued.
pub fn receive(
    conn: &Connection,
    recipient: &str,
    cursor: Option<&str>,
    ack_cursor: Option<&str>,
    limit: u32,
) -> Result<AgentMessagePage> {
    if limit == 0 || limit > crate::agent_messaging::MAX_RECEIVE_ITEMS {
        return Err(Error::new(
            "invalid_limit",
            "Receive limit must be 1 to 100.",
        ));
    }
    let tx = conn.unchecked_transaction()?;
    let acknowledged: i64 = tx
        .query_row(
            "SELECT sequence FROM agent_message_ack WHERE recipient=?1",
            [recipient],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let ack = ack_cursor
        .map(|token| cursor_sequence(&tx, recipient, token))
        .transpose()?
        .unwrap_or(acknowledged)
        .max(acknowledged);
    let after = cursor
        .map(|token| cursor_sequence(&tx, recipient, token))
        .transpose()?
        .unwrap_or(ack);
    // Select only available work. Scheduler-owned entries never reach model
    // receive; a successful receiver claim excludes any future scheduler claim.
    let rows: Vec<(i64, String)> = {
        let mut statement = tx.prepare(
            "SELECT a.sequence,a.interaction_id FROM agent_message_availability a
            JOIN interactions i ON i.id=a.interaction_id
            LEFT JOIN agent_message_delivery d ON d.interaction_id=i.id
            WHERE a.recipient=?1 AND a.sequence>?2
            AND (d.owner IS NULL OR d.owner IN ('stored','pending','receiver_available'))
            ORDER BY a.sequence LIMIT ?3",
        )?;
        let rows = statement.query_map(params![recipient, after, limit + 1], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut has_more = rows.len() > limit as usize;
    let mut sequence = after;
    let mut messages = Vec::new();
    let mut body_bytes = 0;
    // Reserve the fixed page/operation envelope and both opaque cursor tokens.
    let mut serialized_bytes = 1024;
    for (seq, id) in rows.into_iter().take(limit as usize) {
        let record = load(&tx, &id)?;
        let InteractionBodyRef::Inline { body } = record.body_ref else {
            return Err(Error::new(
                "invalid_message",
                "V2 body is not inline canonical text.",
            ));
        };
        let reply_status = if record.kind == InteractionKind::Reply {
            let status: String = tx.query_row(
                "SELECT status FROM structured_replies WHERE request_id=?1",
                [record.parent_interaction_id.as_deref().unwrap_or("")],
                |row| row.get(0),
            )?;
            Some(super::enum_from_value(&status)?)
        } else {
            None
        };
        let message = AgentMessage {
            interaction_id: id.clone(),
            kind: record.kind,
            sender: record.sender_session_id.unwrap_or_default(),
            message: body,
            parent_interaction_id: record.parent_interaction_id,
            reply_status,
            created_at: record.created_at,
        };
        let encoded_bytes = serde_json::to_vec(&message)
            .map_err(|error| Error::new("storage_error", error.to_string()))?
            .len()
            + 1;
        if body_bytes + message.message.len() > crate::agent_messaging::MAX_RECEIVE_BODY_BYTES
            || serialized_bytes + encoded_bytes
                > crate::agent_messaging::MAX_RECEIVE_SERIALIZED_BYTES
        {
            if messages.is_empty() {
                return Err(Error::new(
                    "invalid_message",
                    "Stored message exceeds receive serialization budget.",
                ));
            }
            has_more = true;
            break;
        }
        // A budget-deferred entry remains unclaimed and does not advance the cursor.
        tx.execute("UPDATE agent_message_delivery SET owner='receiver_available',claim_token=?2 WHERE interaction_id=?1 AND owner IN ('stored','pending')",params![id,new_id("claim")])?;
        body_bytes += message.message.len();
        serialized_bytes += encoded_bytes;
        messages.push(message);
        sequence = seq;
    }
    let token = issue_cursor(&tx, recipient, sequence)?;
    if ack_cursor.is_some() {
        tx.execute("INSERT INTO agent_message_ack(recipient,sequence) VALUES(?1,?2) ON CONFLICT(recipient) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)",params![recipient,ack])?;
    }
    tx.commit()?;
    Ok(AgentMessagePage {
        messages,
        next_cursor: token.clone(),
        ack_cursor: token,
        has_more,
        timed_out: false,
        wake_reason: None,
    })
}

/// Durable scheduler claim made only while the runtime caller holds generation
/// ownership. No provider I/O may precede this commit.
pub struct TaskClaim {
    pub record: InteractionRecord,
    pub token: String,
    pub generation: u64,
}

pub fn claim_next_task(
    conn: &Connection,
    recipient: &str,
    generation: u64,
) -> Result<Option<TaskClaim>> {
    let tx = conn.unchecked_transaction()?;
    let id: Option<String> = tx.query_row("SELECT d.interaction_id FROM agent_message_delivery d JOIN agent_message_availability a ON a.interaction_id=d.interaction_id JOIN interactions i ON i.id=d.interaction_id
        WHERE d.recipient=?1 AND d.operation='followup_task' AND d.owner='pending' AND i.status='awaiting_reply' ORDER BY a.sequence LIMIT 1",[recipient],|row|row.get(0)).optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    let token = new_id("claim");
    let changed = tx.execute("UPDATE agent_message_delivery SET owner='dispatching',generation=?2,claim_token=?3 WHERE interaction_id=?1 AND owner='pending'",params![id,generation,token])?;
    if changed != 1 {
        return Ok(None);
    }
    let record = load(&tx, &id)?;
    tx.commit()?;
    Ok(Some(TaskClaim {
        record,
        token,
        generation,
    }))
}

/// A dispatched claim is never returned to receive automatically. A lost
/// process/receipt remains dispatching or uncertain and cannot authorize replay.
pub fn finish_claim(conn: &Connection, claim: &TaskClaim, state: &str) -> Result<()> {
    if !matches!(
        state,
        "provider_visible"
            | "provider_accepted"
            | "provider_completed"
            | "uncertain"
            | "failed_before_submit"
    ) {
        return Err(Error::new("invalid_state", "Invalid scheduler result."));
    }
    let changed = conn.execute("UPDATE agent_message_delivery SET owner=?4 WHERE interaction_id=?1 AND claim_token=?2 AND generation=?3 AND owner='dispatching'",params![claim.record.id,claim.token,claim.generation,state])?;
    if changed != 1 {
        return Err(Error::new(
            "stale_claim",
            "Scheduler claim no longer owns this task.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
