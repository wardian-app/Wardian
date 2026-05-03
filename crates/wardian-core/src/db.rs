use crate::paths::state_db_path;
use once_cell::sync::Lazy;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Arc, Mutex};

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

    let mut global_conn = DB_CONN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *global_conn = Some(conn);
    Ok(())
}

pub fn get_db_conn<F, T>(f: F) -> Result<T, Box<dyn std::error::Error>>
where
    F: FnOnce(&Connection) -> Result<T, Box<dyn std::error::Error>>,
{
    let guard = DB_CONN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    let last_status: Option<String> = conn
        .query_row(
            "SELECT last_status FROM agents WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();

    if last_status.as_deref() != Some(status) {
        let timestamp =
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE agents
             SET last_status = ?1,
                 last_pid = COALESCE(?2, last_pid),
                 last_status_at = ?3
             WHERE session_id = ?4",
            params![status, pid, timestamp, session_id],
        )?;

        conn.execute(
            "INSERT INTO events (session_id, event_type, payload) VALUES (?1, ?2, ?3)",
            params![session_id, "status_change", status],
        )?;
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
}
