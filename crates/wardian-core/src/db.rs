use crate::paths::state_db_path;
pub mod agent_messaging;
use once_cell::sync::Lazy;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Arc, Mutex};

use crate::control::{
    DeliveryErrorDetail, DeliveryTransportKind, InteractionBodyRef,
    InteractionDeliveryAttemptRecord, InteractionKind, InteractionRecord, InteractionStatus,
    InteractionTriggerPolicy, MailboxDeliveryPhase, MailboxMessageRecord, MailboxMessageStatus,
    MessageInputMode, MessageOrigin, ProviderInputReadiness, ProviderInputState,
    ProviderReadyEvidence, QueuePolicy, ReplyStatus, StructuredReply,
};
use crate::native_transport::{
    NativeDeliveryEvidence, NativeDeliveryRecord, NativeMessageOperation, NativeSessionBinding,
};

static DB_CONN: Lazy<Arc<Mutex<Option<Connection>>>> = Lazy::new(|| Arc::new(Mutex::new(None)));

#[derive(Debug, Clone)]
pub struct AgentUpsert<'a> {
    pub session_id: &'a str,
    pub session_name: &'a str,
    pub description: &'a str,
    pub agent_class: &'a str,
    pub provider: &'a str,
    pub workspace: Option<&'a str>,
    pub project: Option<&'a str>,
    pub is_off: bool,
    pub created_at: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub session_id: String,
    pub session_name: String,
    pub description: String,
    pub agent_class: Option<String>,
    pub provider: Option<String>,
    pub workspace: Option<String>,
    pub project: Option<String>,
    pub last_status: Option<String>,
    pub last_pid: Option<u32>,
    pub is_off: bool,
    pub created_at: Option<String>,
    pub last_status_at: Option<String>,
}

/// The subset of a user message interaction needed to hydrate Last-Queried
/// telemetry without decoding the interaction body or delivery state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessageTimestampRecord {
    pub target_session_ids: Vec<String>,
    pub created_at: String,
}

/// Durable provider-derived query time used to hydrate telemetry after a
/// restart, including sessions whose provider log has grown beyond the
/// bounded recovery window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentQueryTimestampRecord {
    pub session_id: String,
    pub last_query_timestamp: String,
}

pub fn init_db() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = state_db_path().ok_or("could not resolve Wardian state.db path")?;
    init_db_at_path(&db_path)
}

pub fn init_db_at_path(db_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    run_migrations(&conn)?;

    let mut global_conn = DB_CONN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *global_conn = Some(conn);
    Ok(())
}

pub fn get_db_conn<F, T>(f: F) -> Result<T, Box<dyn std::error::Error>>
where
    F: FnOnce(&Connection) -> Result<T, Box<dyn std::error::Error>>,
{
    let guard = DB_CONN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(ref conn) = *guard {
        f(conn)
    } else {
        Err("database not initialized".into())
    }
}

pub fn run_migrations(conn: &Connection) -> rusqlite::Result<()> {
    // Startup can race another app/CLI connection while it enables WAL or
    // reaches the telemetry migration lease. Wait briefly for that owner
    // rather than turning a normal concurrent launch into SQLITE_BUSY.
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agents (
            session_id TEXT PRIMARY KEY,
            session_name TEXT UNIQUE,
            description TEXT NOT NULL DEFAULT '',
            agent_class TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            is_off BOOLEAN DEFAULT 0,
            last_status TEXT,
            last_pid INTEGER,
            provider TEXT,
            workspace TEXT,
            project TEXT,
            last_status_at DATETIME,
            last_query_timestamp TEXT
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT,
            event_type TEXT,
            payload TEXT,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(session_id) REFERENCES agents(session_id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS interactions (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            sender_session_id TEXT,
            target_session_ids TEXT NOT NULL,
            status TEXT NOT NULL,
            trigger_policy TEXT NOT NULL,
            body_ref TEXT NOT NULL,
            parent_interaction_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_interactions_user_message_created_at
         ON interactions(created_at, id, target_session_ids)
         WHERE kind = 'message' AND sender_session_id IS NULL",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_interactions_created_at
         ON interactions(created_at DESC, id DESC)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS interaction_delivery_attempts (
            id TEXT PRIMARY KEY,
            interaction_id TEXT NOT NULL,
            target_session_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            runtime_state TEXT NOT NULL,
            delivery_state TEXT NOT NULL,
            delivery_phase TEXT,
            observed_state TEXT,
            reason TEXT,
            error_code TEXT,
            error_message TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(interaction_id) REFERENCES interactions(id)
        )",
        [],
    )?;
    ensure_column(
        conn,
        "interaction_delivery_attempts",
        "transport",
        "TEXT NOT NULL DEFAULT 'live_surface'",
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS mailbox_messages (
            id TEXT PRIMARY KEY,
            interaction_id TEXT NOT NULL,
            target_session_id TEXT NOT NULL,
            body TEXT NOT NULL,
            input_mode TEXT NOT NULL,
            queue_policy TEXT NOT NULL,
            approval_action TEXT,
            origin TEXT,
            created_at TEXT NOT NULL,
            status TEXT NOT NULL,
            phase TEXT NOT NULL,
            FOREIGN KEY(interaction_id) REFERENCES interactions(id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_mailbox_messages_target_status
         ON mailbox_messages(target_session_id, status, created_at, id)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS interaction_events (
            event_id TEXT PRIMARY KEY,
            interaction_id TEXT,
            session_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            generation INTEGER NOT NULL,
            source TEXT NOT NULL,
            payload TEXT NOT NULL,
            occurred_at TEXT NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS provider_input_state (
            session_id TEXT PRIMARY KEY,
            generation INTEGER NOT NULL,
            state TEXT NOT NULL,
            ready_evidence TEXT,
            observed_at TEXT NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS structured_replies (
            request_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            body TEXT NOT NULL,
            target_session_id TEXT NOT NULL,
            source_session_id TEXT,
            replied_at TEXT NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS native_deliveries (
            interaction_id TEXT PRIMARY KEY,
            target_agent_id TEXT NOT NULL,
            sender_agent_id TEXT,
            operation TEXT NOT NULL,
            caller_idempotency_key TEXT,
            canonical_hash TEXT NOT NULL,
            generation INTEGER NOT NULL,
            phase TEXT NOT NULL,
            record_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_native_delivery_idempotency
         ON native_deliveries(
            COALESCE(sender_agent_id, ''), target_agent_id, operation, caller_idempotency_key
         )
         WHERE caller_idempotency_key IS NOT NULL",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_native_delivery_queue
         ON native_deliveries(target_agent_id, phase, created_at, interaction_id)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS native_session_bindings (
            target_agent_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            provider TEXT NOT NULL,
            transport TEXT NOT NULL,
            binding_json TEXT NOT NULL,
            observed_at TEXT NOT NULL,
            PRIMARY KEY(target_agent_id, generation)
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS native_delivery_evidence (
            event_id TEXT PRIMARY KEY,
            interaction_id TEXT NOT NULL,
            phase TEXT NOT NULL,
            evidence_json TEXT NOT NULL,
            observed_at TEXT NOT NULL,
            FOREIGN KEY(interaction_id) REFERENCES native_deliveries(interaction_id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_native_delivery_evidence_interaction
         ON native_delivery_evidence(interaction_id, observed_at, event_id)",
        [],
    )?;

    for (name, definition) in [
        ("provider", "TEXT"),
        ("workspace", "TEXT"),
        ("project", "TEXT"),
        ("last_status_at", "DATETIME"),
        ("last_query_timestamp", "TEXT"),
        ("description", "TEXT NOT NULL DEFAULT ''"),
    ] {
        ensure_column(conn, "agents", name, definition)?;
    }

    crate::telemetry::run_telemetry_migrations(conn)?;
    agent_messaging::migrate(conn)?;
    Ok(())
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    let columns = table_columns(conn, table)?;
    if columns.iter().any(|existing| existing == column) {
        return Ok(());
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

pub fn table_columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect()
}

pub fn upsert_agent(upsert: &AgentUpsert<'_>) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        upsert_agent_with_conn(conn, upsert)?;
        Ok(())
    })
}

pub fn upsert_agent_with_conn(conn: &Connection, upsert: &AgentUpsert<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO agents (
            session_id,
            session_name,
            description,
            agent_class,
            provider,
            workspace,
            project,
            is_off,
            created_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, COALESCE(?9, CURRENT_TIMESTAMP))
        ON CONFLICT(session_id) DO UPDATE SET
            session_name = excluded.session_name,
            description = excluded.description,
            agent_class = excluded.agent_class,
            provider = excluded.provider,
            workspace = excluded.workspace,
            project = excluded.project,
            is_off = excluded.is_off",
        params![
            upsert.session_id,
            upsert.session_name,
            upsert.description,
            upsert.agent_class,
            upsert.provider,
            upsert.workspace,
            upsert.project,
            upsert.is_off,
            upsert.created_at,
        ],
    )?;
    Ok(())
}

pub fn update_agent_status(
    session_id: &str,
    status: &str,
    pid: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        update_agent_status_with_conn(conn, session_id, status, pid)?;
        Ok(())
    })
}

pub fn update_agent_status_with_conn(
    conn: &Connection,
    session_id: &str,
    status: &str,
    pid: Option<u32>,
) -> rusqlite::Result<()> {
    let current: Option<(Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT last_status, last_pid FROM agents WHERE session_id = ?1",
            params![session_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            },
        )
        .optional()?;
    let (last_status, last_pid) = current.unwrap_or((None, None));

    let should_clear_pid = pid.is_none() && status == "Off";
    let pid_changed = pid
        .map(i64::from)
        .is_some_and(|next_pid| Some(next_pid) != last_pid);

    if last_status.as_deref() != Some(status) || pid_changed || should_clear_pid {
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE agents
             SET last_status = ?1,
                 last_pid = CASE WHEN ?3 THEN NULL ELSE COALESCE(?2, last_pid) END,
                 last_status_at = ?4
             WHERE session_id = ?5",
            params![status, pid, should_clear_pid, timestamp, session_id],
        )?;

        if last_status.as_deref() != Some(status) {
            conn.execute(
                "INSERT INTO events (session_id, event_type, payload) VALUES (?1, ?2, ?3)",
                params![session_id, "status_change", status],
            )?;
        }
    }
    Ok(())
}

/// Persists the newest provider-derived user-message timestamp for an agent.
/// SQLite's Julian-day comparison handles the RFC3339 offsets emitted by the
/// supported providers while keeping an older observation from regressing the
/// durable watermark.
pub fn update_agent_query_timestamp(
    session_id: &str,
    timestamp: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        update_agent_query_timestamp_with_conn(conn, session_id, timestamp)?;
        Ok(())
    })
}

pub fn update_agent_query_timestamp_with_conn(
    conn: &Connection,
    session_id: &str,
    timestamp: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE agents
         SET last_query_timestamp = ?1
         WHERE session_id = ?2
           AND (
               last_query_timestamp IS NULL
               OR julianday(?1) > julianday(last_query_timestamp)
           )",
        params![timestamp, session_id],
    )?;
    Ok(())
}

pub fn list_agent_query_timestamp_records(
) -> Result<Vec<AgentQueryTimestampRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| Ok(list_agent_query_timestamp_records_with_conn(conn)?))
}

pub fn list_agent_query_timestamp_records_with_conn(
    conn: &Connection,
) -> rusqlite::Result<Vec<AgentQueryTimestampRecord>> {
    let mut statement = conn.prepare(
        "SELECT session_id, last_query_timestamp
         FROM agents
         WHERE last_query_timestamp IS NOT NULL",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(AgentQueryTimestampRecord {
            session_id: row.get(0)?,
            last_query_timestamp: row.get(1)?,
        })
    })?;
    rows.collect()
}

pub fn record_event(
    session_id: &str,
    event_type: &str,
    payload: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.execute(
            "INSERT INTO events (session_id, event_type, payload) VALUES (?1, ?2, ?3)",
            params![session_id, event_type, payload],
        )?;
        Ok(())
    })
}

pub fn delete_agent(session_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut guard = DB_CONN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let conn = guard.as_mut().ok_or("database not initialized")?;
    delete_agent_with_conn(conn, session_id)?;
    Ok(())
}

fn delete_agent_with_conn(conn: &mut Connection, session_id: &str) -> rusqlite::Result<()> {
    let transaction = conn.transaction()?;
    let interaction_ids = {
        let mut statement = transaction.prepare(
            "SELECT id, sender_session_id, target_session_ids, parent_interaction_id
             FROM interactions",
        )?;
        let mut rows = statement.query([])?;
        let mut candidates = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let sender: Option<String> = row.get(1)?;
            let targets_json: String = row.get(2)?;
            let targets: Vec<String> = serde_json::from_str(&targets_json).map_err(to_sql_error)?;
            let parent_interaction_id: Option<String> = row.get(3)?;
            candidates.push((id, sender, targets, parent_interaction_id));
        }

        let mut ids = candidates
            .iter()
            .filter(|(_, sender, targets, _)| {
                sender.as_deref() == Some(session_id)
                    || targets.iter().any(|target| target == session_id)
            })
            .map(|(id, _, _, _)| id.clone())
            .collect::<Vec<_>>();
        let mut known_ids = ids
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        loop {
            let descendants = candidates
                .iter()
                .filter(|(id, _, _, parent)| {
                    !known_ids.contains(id)
                        && parent
                            .as_deref()
                            .is_some_and(|parent_id| known_ids.contains(parent_id))
                })
                .map(|(id, _, _, _)| id.clone())
                .collect::<Vec<_>>();
            if descendants.is_empty() {
                break;
            }
            known_ids.extend(descendants.iter().cloned());
            ids.extend(descendants);
        }
        ids
    };
    transaction.execute(
        "DELETE FROM mailbox_messages WHERE target_session_id = ?1",
        params![session_id],
    )?;
    transaction.execute(
        "DELETE FROM provider_input_state WHERE session_id = ?1",
        params![session_id],
    )?;
    transaction.execute(
        "DELETE FROM structured_replies WHERE target_session_id = ?1",
        params![session_id],
    )?;
    transaction.execute(
        "DELETE FROM interaction_delivery_attempts WHERE target_session_id = ?1",
        params![session_id],
    )?;
    transaction.execute(
        "DELETE FROM interaction_events WHERE session_id = ?1",
        params![session_id],
    )?;
    transaction.execute(
        "DELETE FROM native_delivery_evidence WHERE interaction_id IN (
            SELECT interaction_id FROM native_deliveries WHERE target_agent_id = ?1 OR sender_agent_id = ?1
         )",
        params![session_id],
    )?;
    transaction.execute(
        "DELETE FROM native_deliveries WHERE target_agent_id = ?1 OR sender_agent_id = ?1",
        params![session_id],
    )?;
    transaction.execute(
        "DELETE FROM native_session_bindings WHERE target_agent_id = ?1",
        params![session_id],
    )?;
    agent_messaging::delete_references(&transaction, session_id, &interaction_ids)?;
    for interaction_id in interaction_ids {
        transaction.execute(
            "DELETE FROM mailbox_messages WHERE interaction_id = ?1",
            params![interaction_id],
        )?;
        transaction.execute(
            "DELETE FROM structured_replies WHERE request_id = ?1",
            params![interaction_id],
        )?;
        transaction.execute(
            "DELETE FROM interaction_delivery_attempts WHERE interaction_id = ?1",
            params![interaction_id],
        )?;
        transaction.execute(
            "DELETE FROM interaction_events WHERE interaction_id = ?1",
            params![interaction_id],
        )?;
        transaction.execute(
            "DELETE FROM native_delivery_evidence WHERE interaction_id = ?1",
            params![interaction_id],
        )?;
        transaction.execute(
            "DELETE FROM native_deliveries WHERE interaction_id = ?1",
            params![interaction_id],
        )?;
        transaction.execute(
            "DELETE FROM interactions WHERE id = ?1",
            params![interaction_id],
        )?;
    }
    transaction.execute(
        "DELETE FROM events WHERE session_id = ?1",
        params![session_id],
    )?;
    transaction.execute(
        "DELETE FROM agents WHERE session_id = ?1",
        params![session_id],
    )?;
    transaction.commit()
}

pub fn get_agent_by_session_id_with_conn(
    conn: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<AgentRow>> {
    conn.query_row(
        agent_select_sql("WHERE session_id = ?1").as_str(),
        params![session_id],
        row_to_agent,
    )
    .optional()
}

pub fn get_all_agents() -> Result<Vec<AgentRow>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| Ok(get_all_agents_with_conn(conn)?))
}

pub fn upsert_interaction_record(
    record: &InteractionRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        upsert_interaction_record_with_conn(conn, record)?;
        Ok(())
    })
}

/// Persist related interaction records together so parent state and its reply do
/// not become independently visible after a partial approval resolution.
pub fn upsert_interaction_records(
    records: &[InteractionRecord],
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let transaction = conn.unchecked_transaction()?;
        for record in records {
            upsert_interaction_record_with_conn(&transaction, record)?;
        }
        transaction.commit()?;
        Ok(())
    })
}

pub fn upsert_interaction_record_with_conn(
    conn: &Connection,
    record: &InteractionRecord,
) -> rusqlite::Result<()> {
    let target_session_ids =
        serde_json::to_string(&record.target_session_ids).map_err(to_sql_error)?;
    let body_ref = serde_json::to_string(&record.body_ref).map_err(to_sql_error)?;
    conn.execute(
        "INSERT INTO interactions (
            id,
            kind,
            sender_session_id,
            target_session_ids,
            status,
            trigger_policy,
            body_ref,
            parent_interaction_id,
            created_at,
            updated_at,
            completed_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(id) DO UPDATE SET
            kind = excluded.kind,
            sender_session_id = excluded.sender_session_id,
            target_session_ids = excluded.target_session_ids,
            status = excluded.status,
            trigger_policy = excluded.trigger_policy,
            body_ref = excluded.body_ref,
            parent_interaction_id = excluded.parent_interaction_id,
            updated_at = excluded.updated_at,
            completed_at = excluded.completed_at",
        params![
            record.id,
            enum_value(&record.kind)?,
            record.sender_session_id,
            target_session_ids,
            enum_value(&record.status)?,
            enum_value(&record.trigger_policy)?,
            body_ref,
            record.parent_interaction_id,
            record.created_at,
            record.updated_at,
            record.completed_at,
        ],
    )?;
    Ok(())
}

pub fn list_interaction_records() -> Result<Vec<InteractionRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let records = list_interaction_records_with_conn(conn)?;
        Ok(records)
    })
}

/// Lists the target IDs and creation times needed for historical Last-Queried
/// telemetry. The filtered projection avoids decoding unrelated interaction
/// kinds and fields on every telemetry pass.
pub fn list_user_message_timestamp_records(
) -> Result<Vec<UserMessageTimestampRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let records = list_user_message_timestamp_records_with_conn(conn)?;
        Ok(records)
    })
}

pub fn list_user_message_timestamp_records_with_conn(
    conn: &Connection,
) -> rusqlite::Result<Vec<UserMessageTimestampRecord>> {
    let mut stmt = conn.prepare(
        "SELECT target_session_ids, created_at
         FROM interactions
         WHERE kind = 'message' AND sender_session_id IS NULL
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let target_session_ids: String = row.get(0)?;
        Ok(UserMessageTimestampRecord {
            target_session_ids: serde_json::from_str(&target_session_ids).map_err(to_sql_error)?,
            created_at: row.get(1)?,
        })
    })?;
    rows.collect()
}

pub fn list_recent_interaction_records(
    limit: usize,
) -> Result<Vec<InteractionRecord>, Box<dyn std::error::Error>> {
    list_recent_interaction_records_page(limit, 0)
}

pub fn list_recent_interaction_records_page(
    limit: usize,
    offset: usize,
) -> Result<Vec<InteractionRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        Ok(list_recent_interaction_records_page_with_conn(
            conn, limit, offset,
        )?)
    })
}

pub fn list_recent_interaction_records_page_with_conn(
    conn: &Connection,
    limit: usize,
    offset: usize,
) -> rusqlite::Result<Vec<InteractionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, sender_session_id, target_session_ids, status, trigger_policy,
            body_ref, parent_interaction_id, created_at, updated_at, completed_at
         FROM interactions
         ORDER BY created_at DESC, id DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map([limit as i64, offset as i64], row_to_interaction_record)?;
    rows.collect()
}

/// Lists interaction records newer than `since`, newest first.
pub fn list_recent_interaction_records_since_page(
    limit: usize,
    offset: usize,
    since: &str,
) -> Result<Vec<InteractionRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        Ok(list_recent_interaction_records_since_page_with_conn(
            conn, limit, offset, since,
        )?)
    })
}

fn list_recent_interaction_records_since_page_with_conn(
    conn: &Connection,
    limit: usize,
    offset: usize,
    since: &str,
) -> rusqlite::Result<Vec<InteractionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, sender_session_id, target_session_ids, status, trigger_policy,
            body_ref, parent_interaction_id, created_at, updated_at, completed_at
         FROM interactions
         WHERE created_at >= ?3
         ORDER BY created_at DESC, id DESC
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map(
        params![limit as i64, offset as i64, since],
        row_to_interaction_record,
    )?;
    rows.collect()
}

/// Lists a bounded page of interaction records of one kind, newest first.
///
/// Callers that project a specific interaction kind should use this helper
/// instead of loading the complete interaction table into memory.
pub fn list_recent_interaction_records_by_kind_with_conn(
    conn: &Connection,
    kind: &str,
    limit: usize,
    offset: usize,
) -> rusqlite::Result<Vec<InteractionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, sender_session_id, target_session_ids, status, trigger_policy,
            body_ref, parent_interaction_id, created_at, updated_at, completed_at
         FROM interactions
         WHERE kind = ?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![kind, limit as i64, offset as i64],
        row_to_interaction_record,
    )?;
    rows.collect()
}

/// Lists replies for one notification without loading unrelated interaction
/// history. The first reply is the durable decision for that notification.
pub fn list_interaction_replies_for_parent_with_conn(
    conn: &Connection,
    parent_interaction_id: &str,
) -> rusqlite::Result<Vec<InteractionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, sender_session_id, target_session_ids, status, trigger_policy,
            body_ref, parent_interaction_id, created_at, updated_at, completed_at
         FROM interactions
         WHERE kind = 'reply' AND parent_interaction_id = ?1
         ORDER BY created_at ASC, id ASC
         LIMIT 1",
    )?;
    let rows = stmt.query_map([parent_interaction_id], row_to_interaction_record)?;
    rows.collect()
}

pub fn list_interaction_records_with_conn(
    conn: &Connection,
) -> rusqlite::Result<Vec<InteractionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, sender_session_id, target_session_ids, status, trigger_policy,
            body_ref, parent_interaction_id, created_at, updated_at, completed_at
         FROM interactions",
    )?;
    let rows = stmt.query_map([], row_to_interaction_record)?;
    rows.collect()
}

fn row_to_interaction_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<InteractionRecord> {
    let kind: String = row.get(1)?;
    let target_session_ids: String = row.get(3)?;
    let status: String = row.get(4)?;
    let trigger_policy: String = row.get(5)?;
    let body_ref: String = row.get(6)?;
    Ok(InteractionRecord {
        id: row.get(0)?,
        kind: enum_from_value::<InteractionKind>(&kind)?,
        sender_session_id: row.get(2)?,
        target_session_ids: serde_json::from_str(&target_session_ids).map_err(to_sql_error)?,
        status: enum_from_value::<InteractionStatus>(&status)?,
        trigger_policy: enum_from_value::<InteractionTriggerPolicy>(&trigger_policy)?,
        body_ref: serde_json::from_str::<InteractionBodyRef>(&body_ref).map_err(to_sql_error)?,
        parent_interaction_id: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        completed_at: row.get(10)?,
    })
}

pub fn upsert_mailbox_message(
    record: &MailboxMessageRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        upsert_mailbox_message_with_conn(conn, record)?;
        Ok(())
    })
}

pub fn upsert_mailbox_message_with_conn(
    conn: &Connection,
    record: &MailboxMessageRecord,
) -> rusqlite::Result<()> {
    let approval_action = record
        .approval_action
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(to_sql_error)?;
    let origin = record
        .origin
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(to_sql_error)?;
    conn.execute(
        "INSERT INTO mailbox_messages (
            id,
            interaction_id,
            target_session_id,
            body,
            input_mode,
            queue_policy,
            approval_action,
            origin,
            created_at,
            status,
            phase
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(id) DO UPDATE SET
            interaction_id = excluded.interaction_id,
            target_session_id = excluded.target_session_id,
            body = excluded.body,
            input_mode = excluded.input_mode,
            queue_policy = excluded.queue_policy,
            approval_action = excluded.approval_action,
            origin = excluded.origin,
            status = excluded.status,
            phase = excluded.phase",
        params![
            record.id,
            record.interaction_id,
            record.target_session_id,
            record.body,
            enum_value(&record.input_mode)?,
            enum_value(&record.queue_policy)?,
            approval_action,
            origin,
            record.created_at,
            enum_value(&record.status)?,
            enum_value(&record.phase)?,
        ],
    )?;
    Ok(())
}

pub fn list_mailbox_messages() -> Result<Vec<MailboxMessageRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| Ok(list_mailbox_messages_with_conn(conn)?))
}

pub fn list_mailbox_messages_with_conn(
    conn: &Connection,
) -> rusqlite::Result<Vec<MailboxMessageRecord>> {
    let mut stmt = conn.prepare(
        "SELECT
            id,
            interaction_id,
            target_session_id,
            body,
            input_mode,
            queue_policy,
            approval_action,
            origin,
            created_at,
            status,
            phase
         FROM mailbox_messages
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map([], row_to_mailbox_message)?;
    rows.collect()
}

fn row_to_mailbox_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailboxMessageRecord> {
    let input_mode: String = row.get(4)?;
    let queue_policy: String = row.get(5)?;
    let approval_action: Option<String> = row.get(6)?;
    let origin: Option<String> = row.get(7)?;
    let status: String = row.get(9)?;
    let phase: String = row.get(10)?;
    Ok(MailboxMessageRecord {
        id: row.get(0)?,
        interaction_id: row.get(1)?,
        target_session_id: row.get(2)?,
        body: row.get(3)?,
        input_mode: enum_from_value::<MessageInputMode>(&input_mode)?,
        queue_policy: enum_from_value::<QueuePolicy>(&queue_policy)?,
        approval_action: approval_action
            .map(|value| serde_json::from_str(&value).map_err(to_sql_error))
            .transpose()?,
        origin: origin
            .map(|value| serde_json::from_str::<MessageOrigin>(&value).map_err(to_sql_error))
            .transpose()?,
        created_at: row.get(8)?,
        status: enum_from_value::<MailboxMessageStatus>(&status)?,
        phase: enum_from_value::<MailboxDeliveryPhase>(&phase)?,
    })
}

pub fn delete_mailbox_message(id: &str) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.execute("DELETE FROM mailbox_messages WHERE id = ?1", params![id])?;
        Ok(())
    })
}

pub fn delete_mailbox_messages_for_target(
    target_session_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.execute(
            "DELETE FROM mailbox_messages WHERE target_session_id = ?1",
            params![target_session_id],
        )?;
        Ok(())
    })
}

pub fn upsert_interaction_delivery_attempt(
    record: &InteractionDeliveryAttemptRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        upsert_interaction_delivery_attempt_with_conn(conn, record)?;
        Ok(())
    })
}

pub fn upsert_interaction_delivery_attempt_with_conn(
    conn: &Connection,
    record: &InteractionDeliveryAttemptRecord,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO interaction_delivery_attempts (
            id,
            interaction_id,
            target_session_id,
            transport,
            generation,
            runtime_state,
            delivery_state,
            delivery_phase,
            observed_state,
            reason,
            error_code,
            error_message,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
        ON CONFLICT(id) DO UPDATE SET
            transport = excluded.transport,
            generation = excluded.generation,
            runtime_state = excluded.runtime_state,
            delivery_state = excluded.delivery_state,
            delivery_phase = excluded.delivery_phase,
            observed_state = excluded.observed_state,
            reason = excluded.reason,
            error_code = excluded.error_code,
            error_message = excluded.error_message,
            updated_at = excluded.updated_at",
        params![
            record.id,
            record.interaction_id,
            record.target_session_id,
            enum_value(&record.transport)?,
            record.generation as i64,
            record.runtime_state,
            record.delivery_state,
            record.delivery_phase,
            record.observed_state,
            record.reason,
            record.error.as_ref().map(|error| error.code.as_str()),
            record.error.as_ref().map(|error| error.message.as_str()),
            record.created_at,
            record.updated_at,
        ],
    )?;
    Ok(())
}

pub fn list_interaction_delivery_attempts(
    interaction_id: &str,
) -> Result<Vec<InteractionDeliveryAttemptRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        Ok(list_interaction_delivery_attempts_with_conn(
            conn,
            interaction_id,
        )?)
    })
}

pub fn list_interaction_delivery_attempts_with_conn(
    conn: &Connection,
    interaction_id: &str,
) -> rusqlite::Result<Vec<InteractionDeliveryAttemptRecord>> {
    let mut stmt = conn.prepare(
        "SELECT
            id,
            interaction_id,
            target_session_id,
            transport,
            generation,
            runtime_state,
            delivery_state,
            delivery_phase,
            observed_state,
            reason,
            error_code,
            error_message,
            created_at,
            updated_at
         FROM interaction_delivery_attempts
         WHERE interaction_id = ?1
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![interaction_id], row_to_delivery_attempt)?;
    rows.collect()
}

fn row_to_delivery_attempt(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<InteractionDeliveryAttemptRecord> {
    let transport: String = row.get(3)?;
    let error_code: Option<String> = row.get(10)?;
    let error_message: Option<String> = row.get(11)?;
    let error = error_code.map(|code| DeliveryErrorDetail {
        message: error_message.unwrap_or_else(|| code.clone()),
        code,
    });

    Ok(InteractionDeliveryAttemptRecord {
        id: row.get(0)?,
        interaction_id: row.get(1)?,
        target_session_id: row.get(2)?,
        transport: enum_from_value::<DeliveryTransportKind>(&transport)?,
        generation: row.get::<_, i64>(4)?.max(0) as u64,
        runtime_state: row.get(5)?,
        delivery_state: row.get(6)?,
        delivery_phase: row.get(7)?,
        observed_state: row.get(8)?,
        reason: row.get(9)?,
        error,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

pub fn upsert_structured_reply(reply: &StructuredReply) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        upsert_structured_reply_with_conn(conn, reply)?;
        Ok(())
    })
}

pub fn upsert_structured_reply_with_conn(
    conn: &Connection,
    reply: &StructuredReply,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO structured_replies (
            request_id,
            status,
            body,
            target_session_id,
            source_session_id,
            replied_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(request_id) DO UPDATE SET
            status = excluded.status,
            body = excluded.body,
            target_session_id = excluded.target_session_id,
            source_session_id = excluded.source_session_id,
            replied_at = excluded.replied_at",
        params![
            reply.request_id,
            enum_value(&reply.status)?,
            reply.body,
            reply.target_session_id,
            reply.source_session_id,
            reply.replied_at,
        ],
    )?;
    Ok(())
}

pub fn list_structured_replies() -> Result<Vec<StructuredReply>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let replies = list_structured_replies_with_conn(conn)?;
        Ok(replies)
    })
}

pub fn list_structured_replies_with_conn(
    conn: &Connection,
) -> rusqlite::Result<Vec<StructuredReply>> {
    let mut stmt = conn.prepare(
        "SELECT request_id, status, body, target_session_id, source_session_id, replied_at
         FROM structured_replies",
    )?;
    let rows = stmt.query_map([], |row| {
        let status: String = row.get(1)?;
        Ok(StructuredReply {
            request_id: row.get(0)?,
            status: enum_from_value::<ReplyStatus>(&status)?,
            body: row.get(2)?,
            target_session_id: row.get(3)?,
            source_session_id: row.get(4)?,
            replied_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn upsert_provider_input_state(
    state: &ProviderInputState,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        upsert_provider_input_state_with_conn(conn, state)?;
        Ok(())
    })
}

pub fn upsert_provider_input_state_with_conn(
    conn: &Connection,
    state: &ProviderInputState,
) -> rusqlite::Result<()> {
    let ready_evidence = state
        .ready_evidence
        .map(|evidence| enum_value(&evidence))
        .transpose()?;
    conn.execute(
        "INSERT INTO provider_input_state (
            session_id,
            generation,
            state,
            ready_evidence,
            observed_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(session_id) DO UPDATE SET
            generation = excluded.generation,
            state = excluded.state,
            ready_evidence = excluded.ready_evidence,
            observed_at = excluded.observed_at",
        params![
            state.session_id,
            state.generation as i64,
            enum_value(&state.state)?,
            ready_evidence,
            state.observed_at,
        ],
    )?;
    Ok(())
}

pub fn list_provider_input_states() -> Result<Vec<ProviderInputState>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let states = list_provider_input_states_with_conn(conn)?;
        Ok(states)
    })
}

pub fn list_provider_input_states_with_conn(
    conn: &Connection,
) -> rusqlite::Result<Vec<ProviderInputState>> {
    let mut stmt = conn.prepare(
        "SELECT session_id, generation, state, ready_evidence, observed_at FROM provider_input_state",
    )?;
    let rows = stmt.query_map([], |row| {
        let state: String = row.get(2)?;
        let ready_evidence: Option<String> = row.get(3)?;
        Ok(ProviderInputState {
            session_id: row.get(0)?,
            generation: row.get::<_, i64>(1)? as u64,
            state: enum_from_value::<ProviderInputReadiness>(&state)?,
            ready_evidence: ready_evidence
                .as_deref()
                .map(enum_from_value::<ProviderReadyEvidence>)
                .transpose()?,
            observed_at: row.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn delete_provider_input_state(session_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.execute(
            "DELETE FROM provider_input_state WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    })
}

pub fn upsert_native_delivery(
    record: &NativeDeliveryRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| upsert_native_delivery_with_conn(conn, record).map_err(Into::into))
}

pub fn upsert_native_delivery_with_conn(
    conn: &Connection,
    record: &NativeDeliveryRecord,
) -> rusqlite::Result<()> {
    let record_json = serde_json::to_string(record).map_err(to_sql_error)?;
    conn.execute(
        "INSERT INTO native_deliveries (
            interaction_id, target_agent_id, sender_agent_id, operation,
            caller_idempotency_key, canonical_hash, generation, phase,
            record_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(interaction_id) DO UPDATE SET
            target_agent_id = excluded.target_agent_id,
            sender_agent_id = excluded.sender_agent_id,
            operation = excluded.operation,
            caller_idempotency_key = excluded.caller_idempotency_key,
            canonical_hash = excluded.canonical_hash,
            generation = excluded.generation,
            phase = excluded.phase,
            record_json = excluded.record_json,
            updated_at = excluded.updated_at",
        params![
            record.envelope.interaction_id,
            record.envelope.target_agent_id,
            record.envelope.sender_agent_id,
            enum_value(&record.envelope.operation)?,
            record.envelope.caller_idempotency_key,
            record.canonical_hash,
            record.envelope.generation as i64,
            enum_value(&record.phase)?,
            record_json,
            record.created_at,
            record.updated_at,
        ],
    )?;
    Ok(())
}

pub fn replace_native_delivery(
    superseded: &NativeDeliveryRecord,
    replacement: &NativeDeliveryRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let transaction = conn.unchecked_transaction()?;
        upsert_native_delivery_with_conn(&transaction, superseded)?;
        upsert_native_delivery_with_conn(&transaction, replacement)?;
        transaction.commit()?;
        Ok(())
    })
}

pub fn native_delivery(
    interaction_id: &str,
) -> Result<Option<NativeDeliveryRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| native_delivery_with_conn(conn, interaction_id).map_err(Into::into))
}

pub fn native_delivery_with_conn(
    conn: &Connection,
    interaction_id: &str,
) -> rusqlite::Result<Option<NativeDeliveryRecord>> {
    conn.query_row(
        "SELECT record_json FROM native_deliveries WHERE interaction_id = ?1",
        params![interaction_id],
        |row| {
            let json: String = row.get(0)?;
            serde_json::from_str(&json).map_err(to_sql_error)
        },
    )
    .optional()
}

pub fn native_delivery_by_idempotency(
    sender_agent_id: Option<&str>,
    target_agent_id: &str,
    operation: NativeMessageOperation,
    caller_idempotency_key: &str,
) -> Result<Option<NativeDeliveryRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let operation = enum_value(&operation)?;
        conn.query_row(
            "SELECT record_json FROM native_deliveries
             WHERE COALESCE(sender_agent_id, '') = COALESCE(?1, '')
               AND target_agent_id = ?2
               AND operation = ?3
               AND caller_idempotency_key = ?4",
            params![
                sender_agent_id,
                target_agent_id,
                operation,
                caller_idempotency_key
            ],
            |row| {
                let json: String = row.get(0)?;
                serde_json::from_str(&json).map_err(to_sql_error)
            },
        )
        .optional()
        .map_err(Into::into)
    })
}

pub fn list_native_deliveries_for_target(
    target_agent_id: &str,
    limit: usize,
) -> Result<Vec<NativeDeliveryRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT record_json FROM native_deliveries
             WHERE target_agent_id = ?1
             ORDER BY created_at DESC, interaction_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![target_agent_id, limit as i64], |row| {
            let json: String = row.get(0)?;
            serde_json::from_str(&json).map_err(to_sql_error)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

pub fn list_native_deliveries(
    limit: usize,
) -> Result<Vec<NativeDeliveryRecord>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT record_json FROM native_deliveries
             ORDER BY created_at ASC, interaction_id ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let json: String = row.get(0)?;
            serde_json::from_str(&json).map_err(to_sql_error)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

pub fn upsert_native_session_binding(
    binding: &NativeSessionBinding,
) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let json = serde_json::to_string(binding).map_err(to_sql_error)?;
        conn.execute(
            "INSERT INTO native_session_bindings (
                target_agent_id, generation, provider, transport, binding_json, observed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(target_agent_id, generation) DO UPDATE SET
                provider = excluded.provider,
                transport = excluded.transport,
                binding_json = excluded.binding_json,
                observed_at = excluded.observed_at",
            params![
                binding.target_agent_id,
                binding.generation as i64,
                binding.provider,
                binding.transport,
                json,
                binding.observed_at,
            ],
        )?;
        Ok(())
    })
}

pub fn latest_native_session_binding(
    target_agent_id: &str,
) -> Result<Option<NativeSessionBinding>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.query_row(
            "SELECT binding_json FROM native_session_bindings
             WHERE target_agent_id = ?1 ORDER BY generation DESC LIMIT 1",
            params![target_agent_id],
            |row| {
                let json: String = row.get(0)?;
                serde_json::from_str(&json).map_err(to_sql_error)
            },
        )
        .optional()
        .map_err(Into::into)
    })
}

pub fn append_native_delivery_evidence(
    event_id: &str,
    evidence: &NativeDeliveryEvidence,
) -> Result<bool, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let json = serde_json::to_string(evidence).map_err(to_sql_error)?;
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO native_delivery_evidence (
                event_id, interaction_id, phase, evidence_json, observed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event_id,
                evidence.interaction_id,
                enum_value(&evidence.phase)?,
                json,
                evidence.observed_at,
            ],
        )?;
        Ok(inserted == 1)
    })
}

pub fn list_native_delivery_evidence(
    interaction_id: &str,
    limit: usize,
) -> Result<Vec<NativeDeliveryEvidence>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT evidence_json FROM native_delivery_evidence
             WHERE interaction_id = ?1
             ORDER BY observed_at ASC, event_id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![interaction_id, limit as i64], |row| {
            let json: String = row.get(0)?;
            serde_json::from_str(&json).map_err(to_sql_error)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

fn enum_value<T: serde::Serialize>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_value(value)
        .map_err(to_sql_error)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| to_sql_error(std::io::Error::other("serialized enum was not a string")))
}

fn enum_from_value<T: serde::de::DeserializeOwned>(value: &str) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(to_sql_error)
}

fn to_sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

pub fn get_all_agents_with_conn(conn: &Connection) -> rusqlite::Result<Vec<AgentRow>> {
    let sql = agent_select_sql("");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_agent)?;
    rows.collect()
}

pub fn prune_events(max_events_per_agent: usize) -> Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.execute(
            "DELETE FROM events WHERE id IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY timestamp DESC) as row_num
                    FROM events
                ) WHERE row_num > ?1
            )",
            params![max_events_per_agent],
        )?;
        Ok(())
    })
}

pub fn project_name_from_workspace(workspace: &str) -> Option<String> {
    std::path::Path::new(workspace)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn agent_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT session_id,
                session_name,
                description,
                agent_class,
                provider,
                workspace,
                project,
                last_status,
                last_pid,
                is_off,
                created_at,
                last_status_at
         FROM agents {where_clause}"
    )
}

fn row_to_agent(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRow> {
    let last_pid: Option<i64> = row.get(8)?;
    Ok(AgentRow {
        session_id: row.get(0)?,
        session_name: row.get(1)?,
        description: row.get(2)?,
        agent_class: row.get(3)?,
        provider: row.get(4)?,
        workspace: row.get(5)?,
        project: row.get(6)?,
        last_status: row.get(7)?,
        last_pid: last_pid.and_then(|pid| u32::try_from(pid).ok()),
        is_off: row.get(9)?,
        created_at: row.get(10)?,
        last_status_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migration_adds_cli_metadata_columns_to_existing_agents_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agents (
                session_id TEXT PRIMARY KEY,
                session_name TEXT UNIQUE,
                agent_class TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                is_off BOOLEAN DEFAULT 0,
                last_status TEXT,
                last_pid INTEGER
            );",
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        let columns = table_columns(&conn, "agents").unwrap();
        assert!(columns.contains(&"provider".to_string()));
        assert!(columns.contains(&"workspace".to_string()));
        assert!(columns.contains(&"project".to_string()));
        assert!(columns.contains(&"last_status_at".to_string()));
        assert!(columns.contains(&"last_query_timestamp".to_string()));
        assert!(columns.contains(&"description".to_string()));
    }

    #[test]
    fn upsert_agent_persists_cli_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        upsert_agent_with_conn(
            &conn,
            &AgentUpsert {
                session_id: "uuid-1",
                session_name: "coder-a1",
                description: "Owns frontend release follow-up",
                agent_class: "Coder",
                provider: "codex",
                workspace: Some("D:/Development/Wardian"),
                project: Some("Wardian"),
                is_off: false,
                created_at: Some("2026-05-03T20:00:00.000Z"),
            },
        )
        .unwrap();

        let row = get_agent_by_session_id_with_conn(&conn, "uuid-1")
            .unwrap()
            .unwrap();
        assert_eq!(row.provider.as_deref(), Some("codex"));
        assert_eq!(row.description, "Owns frontend release follow-up");
        assert_eq!(row.project.as_deref(), Some("Wardian"));
        assert_eq!(row.workspace.as_deref(), Some("D:/Development/Wardian"));
    }

    #[test]
    fn agent_query_timestamp_watermark_keeps_the_newest_observation() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        upsert_agent_with_conn(
            &conn,
            &AgentUpsert {
                session_id: "uuid-query",
                session_name: "query-agent",
                description: "",
                agent_class: "Coder",
                provider: "codex",
                workspace: None,
                project: None,
                is_off: false,
                created_at: None,
            },
        )
        .unwrap();

        update_agent_query_timestamp_with_conn(&conn, "uuid-query", "2026-08-26T12:00:03.000Z")
            .unwrap();
        update_agent_query_timestamp_with_conn(&conn, "uuid-query", "2026-08-26T12:00:01.000Z")
            .unwrap();

        let records = list_agent_query_timestamp_records_with_conn(&conn).unwrap();
        assert_eq!(
            records,
            vec![AgentQueryTimestampRecord {
                session_id: "uuid-query".to_string(),
                last_query_timestamp: "2026-08-26T12:00:03.000Z".to_string(),
            }]
        );
    }

    #[test]
    fn interaction_records_round_trip_through_db() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let record = InteractionRecord {
            id: "ask_001".to_string(),
            kind: InteractionKind::Task,
            sender_session_id: Some("planner-1".to_string()),
            target_session_ids: vec!["agent-1".to_string()],
            status: InteractionStatus::AwaitingReply,
            trigger_policy: InteractionTriggerPolicy::ReplyRequired,
            body_ref: InteractionBodyRef::Inline {
                body: "review".to_string(),
            },
            parent_interaction_id: None,
            created_at: "2026-05-25T00:00:00.000Z".to_string(),
            updated_at: "2026-05-25T00:00:00.000Z".to_string(),
            completed_at: None,
        };

        upsert_interaction_record_with_conn(&conn, &record).unwrap();

        let records = list_interaction_records_with_conn(&conn).unwrap();
        assert_eq!(records, vec![record]);
    }

    #[test]
    fn recent_interaction_page_filters_before_limit() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let make_record = |id: &str, created_at: &str| InteractionRecord {
            id: id.to_string(),
            kind: InteractionKind::Message,
            sender_session_id: Some("sender-1".to_string()),
            target_session_ids: vec!["agent-1".to_string()],
            status: InteractionStatus::Completed,
            trigger_policy: InteractionTriggerPolicy::NotifyOnly,
            body_ref: InteractionBodyRef::Inline {
                body: "activity".to_string(),
            },
            parent_interaction_id: None,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
            completed_at: Some(created_at.to_string()),
        };

        for record in [
            make_record("old-1", "2026-05-25T00:00:02.000Z"),
            make_record("old-2", "2026-05-25T00:00:01.000Z"),
            make_record("recent-1", "2026-05-26T00:00:02.000Z"),
            make_record("recent-2", "2026-05-26T00:00:01.000Z"),
        ] {
            upsert_interaction_record_with_conn(&conn, &record).unwrap();
        }

        let records = list_recent_interaction_records_since_page_with_conn(
            &conn,
            2,
            0,
            "2026-05-26T00:00:00.000Z",
        )
        .unwrap();

        assert_eq!(
            records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            vec!["recent-1", "recent-2"]
        );
    }

    #[test]
    fn user_message_interaction_records_are_filtered_in_sql() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let make_record =
            |id: &str, kind: InteractionKind, sender_session_id: Option<&str>| InteractionRecord {
                id: id.to_string(),
                kind,
                sender_session_id: sender_session_id.map(str::to_string),
                target_session_ids: vec!["agent-1".to_string()],
                status: InteractionStatus::Queued,
                trigger_policy: InteractionTriggerPolicy::StartTurn,
                body_ref: InteractionBodyRef::Inline {
                    body: "prompt".to_string(),
                },
                parent_interaction_id: None,
                created_at: format!("2026-05-25T00:00:0{id}.000Z"),
                updated_at: format!("2026-05-25T00:00:0{id}.000Z"),
                completed_at: None,
            };

        for record in [
            make_record("1", InteractionKind::Task, None),
            make_record("2", InteractionKind::Message, Some("sender-1")),
            make_record("3", InteractionKind::Message, None),
        ] {
            upsert_interaction_record_with_conn(&conn, &record).unwrap();
        }

        let records = list_user_message_timestamp_records_with_conn(&conn).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].target_session_ids, vec!["agent-1"]);
        assert_eq!(records[0].created_at, "2026-05-25T00:00:03.000Z");
    }

    #[test]
    fn provider_input_state_round_trips_through_db() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let state = ProviderInputState {
            session_id: "agent-1".to_string(),
            generation: 7,
            state: ProviderInputReadiness::Ready,
            ready_evidence: Some(ProviderReadyEvidence::ProviderEvent),
            observed_at: "2026-05-25T00:00:00.000Z".to_string(),
        };

        upsert_provider_input_state_with_conn(&conn, &state).unwrap();

        let states = list_provider_input_states_with_conn(&conn).unwrap();
        assert_eq!(states, vec![state]);
    }

    #[test]
    fn mailbox_messages_round_trip_through_db() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        upsert_interaction_record_with_conn(
            &conn,
            &InteractionRecord {
                id: "int_001".to_string(),
                kind: InteractionKind::Message,
                sender_session_id: None,
                target_session_ids: vec!["agent-1".to_string()],
                status: InteractionStatus::Queued,
                trigger_policy: InteractionTriggerPolicy::StartTurn,
                body_ref: InteractionBodyRef::Inline {
                    body: "deliver after the active turn".to_string(),
                },
                parent_interaction_id: None,
                created_at: "2026-08-01T00:00:00.000Z".to_string(),
                updated_at: "2026-08-01T00:00:00.000Z".to_string(),
                completed_at: None,
            },
        )
        .unwrap();
        let record = MailboxMessageRecord {
            id: "msg_0000000000001_000001".to_string(),
            interaction_id: "int_001".to_string(),
            target_session_id: "agent-1".to_string(),
            body: "deliver after the active turn".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: None,
            created_at: "2026-08-01T00:00:00.000Z".to_string(),
            status: MailboxMessageStatus::Pending,
            phase: MailboxDeliveryPhase::Queued,
        };

        upsert_mailbox_message_with_conn(&conn, &record).unwrap();

        assert_eq!(
            list_mailbox_messages_with_conn(&conn).unwrap(),
            vec![record]
        );
    }

    #[test]
    fn structured_replies_round_trip_through_db() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let reply = StructuredReply {
            request_id: "ask_001".to_string(),
            status: ReplyStatus::Blocked,
            body: "blocked".to_string(),
            target_session_id: "agent-1".to_string(),
            source_session_id: Some("agent-1".to_string()),
            replied_at: "2026-05-25T00:00:01.000Z".to_string(),
        };

        upsert_structured_reply_with_conn(&conn, &reply).unwrap();

        let replies = list_structured_replies_with_conn(&conn).unwrap();
        assert_eq!(replies, vec![reply]);
    }

    #[test]
    fn status_update_sets_last_status_at() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        upsert_agent_with_conn(
            &conn,
            &AgentUpsert {
                session_id: "uuid-1",
                session_name: "coder-a1",
                description: "",
                agent_class: "Coder",
                provider: "codex",
                workspace: None,
                project: None,
                is_off: false,
                created_at: None,
            },
        )
        .unwrap();

        update_agent_status_with_conn(&conn, "uuid-1", "Processing...", Some(123)).unwrap();

        let row = get_agent_by_session_id_with_conn(&conn, "uuid-1")
            .unwrap()
            .unwrap();
        assert_eq!(row.last_status.as_deref(), Some("Processing..."));
        assert_eq!(row.last_pid, Some(123));
        assert!(row.last_status_at.is_some());
    }

    #[test]
    fn off_status_clears_stale_pid() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        upsert_agent_with_conn(
            &conn,
            &AgentUpsert {
                session_id: "uuid-1",
                session_name: "coder-a1",
                description: "",
                agent_class: "Coder",
                provider: "codex",
                workspace: None,
                project: None,
                is_off: false,
                created_at: None,
            },
        )
        .unwrap();

        update_agent_status_with_conn(&conn, "uuid-1", "Processing...", Some(123)).unwrap();
        update_agent_status_with_conn(&conn, "uuid-1", "Off", None).unwrap();

        let row = get_agent_by_session_id_with_conn(&conn, "uuid-1")
            .unwrap()
            .unwrap();
        assert_eq!(row.last_status.as_deref(), Some("Off"));
        assert_eq!(row.last_pid, None);
    }

    #[test]
    fn same_status_update_refreshes_changed_pid() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        upsert_agent_with_conn(
            &conn,
            &AgentUpsert {
                session_id: "uuid-1",
                session_name: "coder-a1",
                description: "",
                agent_class: "Coder",
                provider: "codex",
                workspace: None,
                project: None,
                is_off: false,
                created_at: None,
            },
        )
        .unwrap();

        update_agent_status_with_conn(&conn, "uuid-1", "Idle", Some(123)).unwrap();
        update_agent_status_with_conn(&conn, "uuid-1", "Idle", Some(456)).unwrap();

        let row = get_agent_by_session_id_with_conn(&conn, "uuid-1")
            .unwrap()
            .unwrap();
        assert_eq!(row.last_status.as_deref(), Some("Idle"));
        assert_eq!(row.last_pid, Some(456));

        let event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND event_type = ?2",
                params!["uuid-1", "status_change"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_count, 1);
    }

    #[test]
    fn delete_agent_removes_agent_and_events_in_one_transaction() {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        upsert_agent_with_conn(
            &conn,
            &AgentUpsert {
                session_id: "agent-delete",
                session_name: "DeleteMe",
                description: "",
                agent_class: "Coder",
                provider: "mock",
                workspace: None,
                project: None,
                is_off: true,
                created_at: None,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (session_id, event_type, payload) VALUES (?1, ?2, ?3)",
            params!["agent-delete", "status_change", "Off"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO interactions (
                id, kind, sender_session_id, target_session_ids, status,
                trigger_policy, body_ref, created_at, updated_at
            ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                "interaction-delete",
                "message",
                "[\"agent-delete\"]",
                "delivered",
                "start_turn",
                "{\"inline\":{\"body\":\"hello\"}}",
                "2026-08-21T00:00:00Z",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO interaction_delivery_attempts (
                id, interaction_id, target_session_id, generation,
                runtime_state, delivery_state, created_at, updated_at
            ) VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?6)",
            params![
                "attempt-delete",
                "interaction-delete",
                "agent-delete",
                "ready",
                "delivered",
                "2026-08-21T00:00:00Z",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mailbox_messages (
                id, interaction_id, target_session_id, body, input_mode,
                queue_policy, created_at, status, phase
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                "mailbox-delete",
                "interaction-delete",
                "agent-delete",
                "hello",
                "message",
                "queue_if_busy",
                "2026-08-21T00:00:00Z",
                "pending",
                "queued",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO interaction_events (
                event_id, interaction_id, session_id, kind, generation,
                source, payload, occurred_at
            ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)",
            params![
                "event-delete",
                "interaction-delete",
                "agent-delete",
                "delivery",
                "test",
                "{}",
                "2026-08-21T00:00:00Z",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO provider_input_state (
                session_id, generation, state, observed_at
            ) VALUES (?1, 1, ?2, ?3)",
            params!["agent-delete", "ready", "2026-08-21T00:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO structured_replies (
                request_id, status, body, target_session_id, replied_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "reply-delete",
                "completed",
                "done",
                "agent-delete",
                "2026-08-21T00:00:00Z",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO interactions (
                id, kind, sender_session_id, target_session_ids, status,
                trigger_policy, body_ref, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                "interaction-outbound-delete",
                "task",
                "agent-delete",
                "[\"agent-other\"]",
                "awaiting_reply",
                "reply_required",
                "{\"inline\":{\"body\":\"ask\"}}",
                "2026-08-21T00:00:00Z",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mailbox_messages (
                id, interaction_id, target_session_id, body, input_mode,
                queue_policy, created_at, status, phase
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                "mailbox-outbound-delete",
                "interaction-outbound-delete",
                "agent-other",
                "ask",
                "message",
                "queue_if_busy",
                "2026-08-21T00:00:00Z",
                "pending",
                "queued",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO structured_replies (
                request_id, status, body, target_session_id, replied_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "interaction-outbound-delete",
                "completed",
                "answer",
                "agent-other",
                "2026-08-21T00:00:00Z",
            ],
        )
        .unwrap();

        delete_agent_with_conn(&mut conn, "agent-delete").unwrap();

        assert!(get_agent_by_session_id_with_conn(&conn, "agent-delete")
            .unwrap()
            .is_none());
        let event_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1",
                params!["agent-delete"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_count, 0);
        for table in [
            "interactions",
            "interaction_delivery_attempts",
            "mailbox_messages",
            "interaction_events",
            "provider_input_state",
            "structured_replies",
            "native_deliveries",
            "native_session_bindings",
            "native_delivery_evidence",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} should be removed with the agent");
        }
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migrations_create_interaction_tables() {
        let conn = Connection::open_in_memory().unwrap();

        run_migrations(&conn).unwrap();

        for table in [
            "interactions",
            "interaction_delivery_attempts",
            "mailbox_messages",
            "interaction_events",
            "provider_input_state",
            "structured_replies",
            "native_deliveries",
            "native_session_bindings",
            "native_delivery_evidence",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{table} should exist");
        }
    }

    #[test]
    fn delivery_attempt_round_trips_with_transport() {
        let temp = tempfile::tempdir().expect("tempdir");
        init_db_at_path(&temp.path().join("state.db")).expect("init db");
        let interaction = InteractionRecord {
            id: "int_attempt_parent".to_string(),
            kind: InteractionKind::Message,
            sender_session_id: None,
            target_session_ids: vec!["agent-1".to_string()],
            status: InteractionStatus::Delivering,
            trigger_policy: InteractionTriggerPolicy::StartTurn,
            body_ref: InteractionBodyRef::Inline {
                body: "hello".to_string(),
            },
            parent_interaction_id: None,
            created_at: "2026-06-07T00:00:00.000Z".to_string(),
            updated_at: "2026-06-07T00:00:00.000Z".to_string(),
            completed_at: None,
        };
        upsert_interaction_record(&interaction).expect("insert interaction");

        let attempt = InteractionDeliveryAttemptRecord {
            id: "attempt_1".to_string(),
            interaction_id: interaction.id.clone(),
            target_session_id: "agent-1".to_string(),
            transport: DeliveryTransportKind::LiveSurface,
            generation: 1,
            runtime_state: "live_pty_available".to_string(),
            delivery_state: "submit_sent_unconfirmed".to_string(),
            delivery_phase: Some("submit_key_sent".to_string()),
            observed_state: Some("bytes_sent".to_string()),
            reason: None,
            error: None,
            created_at: "2026-06-07T00:00:00.000Z".to_string(),
            updated_at: "2026-06-07T00:00:00.000Z".to_string(),
        };
        upsert_interaction_delivery_attempt(&attempt).expect("insert attempt");

        let attempts =
            list_interaction_delivery_attempts(&interaction.id).expect("list delivery attempts");

        assert_eq!(attempts, vec![attempt]);
    }
}
