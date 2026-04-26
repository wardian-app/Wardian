use crate::utils::fs::get_wardian_home;
use crate::utils::logging::log_debug;
use once_cell::sync::Lazy;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Arc, Mutex};

static DB_CONN: Lazy<Arc<Mutex<Option<Connection>>>> = Lazy::new(|| Arc::new(Mutex::new(None)));

pub fn init_db() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let home = get_wardian_home().ok_or("Could not resolve Wardian home")?;

    let db_path = home.join("state.db");
    let conn = Connection::open(db_path)?;

    // Enable WAL mode for concurrency
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // Create tables
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agents (
            session_id TEXT PRIMARY KEY,
            session_name TEXT UNIQUE,
            agent_class TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            is_off BOOLEAN DEFAULT 0,
            last_status TEXT,
            last_pid INTEGER
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

    let mut global_conn = DB_CONN.lock().unwrap();
    *global_conn = Some(conn);

    log_debug("[DB] SQLite initialized with WAL mode");
    Ok(())
}

pub fn get_db_conn<F, T>(f: F) -> std::result::Result<T, Box<dyn std::error::Error>>
where
    F: FnOnce(&Connection) -> std::result::Result<T, Box<dyn std::error::Error>>,
{
    let guard = DB_CONN.lock().unwrap();
    if let Some(ref conn) = *guard {
        f(conn)
    } else {
        Err("Database not initialized".into())
    }
}

pub fn upsert_agent(
    session_id: &str,
    session_name: &str,
    agent_class: &str,
    is_off: bool,
    created_at: Option<&str>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.execute(
            "INSERT INTO agents (session_id, session_name, agent_class, is_off, created_at)
             VALUES (?1, ?2, ?3, ?4, COALESCE(?5, CURRENT_TIMESTAMP))
             ON CONFLICT(session_id) DO UPDATE SET
                session_name = excluded.session_name,
                agent_class = excluded.agent_class,
                is_off = excluded.is_off",
            params![session_id, session_name, agent_class, is_off, created_at],
        )?;
        Ok(())
    })
}

pub fn update_agent_status(
    session_id: &str,
    status: &str,
    pid: Option<u32>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        // Only write to events table if status actually changed (Deduplication)
        let last_status: Option<String> = conn
            .query_row(
                "SELECT last_status FROM agents WHERE session_id = ?",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;

        if last_status.as_deref() != Some(status) {
            conn.execute(
                "UPDATE agents SET last_status = ?1, last_pid = COALESCE(?2, last_pid) WHERE session_id = ?3",
                params![status, pid, session_id],
            )?;

            conn.execute(
                "INSERT INTO events (session_id, event_type, payload) VALUES (?1, ?2, ?3)",
                params![session_id, "status_change", status],
            )?;
        }
        Ok(())
    })
}

pub fn record_event(
    session_id: &str,
    event_type: &str,
    payload: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.execute(
            "INSERT INTO events (session_id, event_type, payload) VALUES (?1, ?2, ?3)",
            params![session_id, event_type, payload],
        )?;
        Ok(())
    })
}

pub fn delete_agent(session_id: &str) -> std::result::Result<(), Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        conn.execute(
            "DELETE FROM events WHERE session_id = ?",
            params![session_id],
        )?;
        conn.execute(
            "DELETE FROM agents WHERE session_id = ?",
            params![session_id],
        )?;
        Ok(())
    })
}

pub struct AgentRow {
    pub session_id: String,
    pub session_name: String,
    pub last_status: Option<String>,
    pub last_pid: Option<u32>,
    pub is_off: bool,
    pub created_at: Option<String>,
}

pub fn get_all_agents() -> std::result::Result<Vec<AgentRow>, Box<dyn std::error::Error>> {
    get_db_conn(|conn| {
        let mut stmt = conn.prepare("SELECT session_id, session_name, last_status, last_pid, is_off, created_at FROM agents")?;
        let rows = stmt.query_map([], |row| {
            Ok(AgentRow {
                session_id: row.get(0)?,
                session_name: row.get(1)?,
                last_status: row.get(2)?,
                last_pid: row.get(3)?,
                is_off: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    })
}

pub fn prune_events(
    max_events_per_agent: usize,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
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
