use crate::paths::state_db_path;
use once_cell::sync::Lazy;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Arc, Mutex};

use crate::control::{
    InteractionBodyRef, InteractionKind, InteractionRecord, InteractionStatus,
    InteractionTriggerPolicy, ProviderInputReadiness, ProviderInputState, ProviderReadyEvidence,
    ReplyStatus, StructuredReply,
};

static DB_CONN: Lazy<Arc<Mutex<Option<Connection>>>> = Lazy::new(|| Arc::new(Mutex::new(None)));

#[derive(Debug, Clone)]
pub struct AgentUpsert<'a> {
    pub session_id: &'a str,
    pub session_name: &'a str,
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
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agents (
            session_id TEXT PRIMARY KEY,
            session_name TEXT UNIQUE,
            agent_class TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            is_off BOOLEAN DEFAULT 0,
            last_status TEXT,
            last_pid INTEGER,
            provider TEXT,
            workspace TEXT,
            project TEXT,
            last_status_at DATETIME
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

    for (name, definition) in [
        ("provider", "TEXT"),
        ("workspace", "TEXT"),
        ("project", "TEXT"),
        ("last_status_at", "DATETIME"),
    ] {
        ensure_column(conn, "agents", name, definition)?;
    }
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
            agent_class,
            provider,
            workspace,
            project,
            is_off,
            created_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, COALESCE(?8, CURRENT_TIMESTAMP))
        ON CONFLICT(session_id) DO UPDATE SET
            session_name = excluded.session_name,
            agent_class = excluded.agent_class,
            provider = excluded.provider,
            workspace = excluded.workspace,
            project = excluded.project,
            is_off = excluded.is_off",
        params![
            upsert.session_id,
            upsert.session_name,
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
    get_db_conn(|conn| {
        conn.execute(
            "DELETE FROM events WHERE session_id = ?1",
            params![session_id],
        )?;
        conn.execute(
            "DELETE FROM agents WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    })
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
    let last_pid: Option<i64> = row.get(7)?;
    Ok(AgentRow {
        session_id: row.get(0)?,
        session_name: row.get(1)?,
        agent_class: row.get(2)?,
        provider: row.get(3)?,
        workspace: row.get(4)?,
        project: row.get(5)?,
        last_status: row.get(6)?,
        last_pid: last_pid.and_then(|pid| u32::try_from(pid).ok()),
        is_off: row.get(8)?,
        created_at: row.get(9)?,
        last_status_at: row.get(10)?,
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
        assert_eq!(row.project.as_deref(), Some("Wardian"));
        assert_eq!(row.workspace.as_deref(), Some("D:/Development/Wardian"));
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
            "interaction_events",
            "provider_input_state",
            "structured_replies",
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
}
