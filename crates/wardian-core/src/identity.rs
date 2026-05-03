use crate::db::{get_all_agents_with_conn, get_agent_by_session_id_with_conn, AgentRow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentIdentity {
    pub name: String,
    pub uuid: String,
    pub class: String,
    pub provider: String,
    pub project: String,
    pub status: String,
    pub pid: Option<u32>,
    pub started_at: Option<String>,
    pub workspace: Option<String>,
    pub last_status_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scope {
    Project,
    #[default]
    All,
}

#[derive(Debug, Clone, Default)]
pub struct ListFilters {
    pub scope: Scope,
    pub caller_project: Option<String>,
    pub status: Option<String>,
    pub class: Option<String>,
    pub project: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("agent not found: {0}")]
    NotFound(String),
    #[error("WARDIAN_SESSION_ID environment variable is not set")]
    NotInSession,
    #[error("database unavailable: {0}")]
    Db(#[from] rusqlite::Error),
}

pub fn resolve_self(conn: &rusqlite::Connection) -> Result<AgentIdentity, IdentityError> {
    let session_id = std::env::var("WARDIAN_SESSION_ID")
        .map_err(|_| IdentityError::NotInSession)?;
    resolve_by_name_or_uuid(conn, &session_id)
}

pub fn resolve_by_name_or_uuid(
    conn: &rusqlite::Connection,
    value: &str,
) -> Result<AgentIdentity, IdentityError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(IdentityError::NotFound(value.to_string()));
    }

    if let Some(row) = get_agent_by_session_id_with_conn(conn, trimmed)? {
        return Ok(row_to_identity(row));
    }

    get_all_agents_with_conn(conn)?
        .into_iter()
        .find(|row| row.session_name == trimmed)
        .map(row_to_identity)
        .ok_or_else(|| IdentityError::NotFound(trimmed.to_string()))
}

pub fn list_agents(
    conn: &rusqlite::Connection,
    filters: &ListFilters,
) -> Result<Vec<AgentIdentity>, IdentityError> {
    let mut agents = Vec::new();
    for row in get_all_agents_with_conn(conn)? {
        let agent = row_to_identity(row);
        if !matches_status(&agent, filters.status.as_deref()) {
            continue;
        }
        if !matches_optional(&agent.class, filters.class.as_deref()) {
            continue;
        }
        if !matches_optional(&agent.project, filters.project.as_deref()) {
            continue;
        }
        if filters.scope == Scope::Project {
            let Some(caller_project) = filters.caller_project.as_deref() else {
                continue;
            };
            if agent.project != caller_project {
                continue;
            }
        }
        agents.push(agent);
    }
    Ok(agents)
}

pub fn normalize_status(value: &str) -> String {
    let trimmed = value.trim();
    match trimmed {
        "Idle" => "idle".to_string(),
        "Processing..." => "processing".to_string(),
        "Action Needed" => "action_required".to_string(),
        "Off" => "off".to_string(),
        "Headless" => "headless".to_string(),
        _ if trimmed.to_ascii_lowercase().contains("error") => "error".to_string(),
        _ => trimmed
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("_"),
    }
}

fn row_to_identity(row: AgentRow) -> AgentIdentity {
    AgentIdentity {
        name: row.session_name,
        uuid: row.session_id,
        class: row.agent_class.unwrap_or_default(),
        provider: row.provider.unwrap_or_else(|| "claude".to_string()),
        project: row.project.unwrap_or_default(),
        status: row
            .last_status
            .as_deref()
            .map(normalize_status)
            .unwrap_or_else(|| {
                if row.is_off {
                    "off".to_string()
                } else {
                    "unknown".to_string()
                }
            }),
        pid: row.last_pid,
        started_at: row.created_at,
        workspace: row.workspace,
        last_status_at: row.last_status_at,
    }
}

fn matches_status(agent: &AgentIdentity, status: Option<&str>) -> bool {
    status
        .map(normalize_status)
        .is_none_or(|expected| agent.status == expected)
}

fn matches_optional(actual: &str, expected: Option<&str>) -> bool {
    expected.is_none_or(|expected| actual == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        run_migrations, update_agent_status_with_conn, upsert_agent_with_conn, AgentUpsert,
    };
    use rusqlite::Connection;

    fn seed() -> Connection {
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
        upsert_agent_with_conn(
            &conn,
            &AgentUpsert {
                session_id: "uuid-2",
                session_name: "architect-a1",
                agent_class: "Architect",
                provider: "claude",
                workspace: Some("D:/Development/Wardian"),
                project: Some("Wardian"),
                is_off: false,
                created_at: Some("2026-05-03T20:01:00.000Z"),
            },
        )
        .unwrap();
        update_agent_status_with_conn(&conn, "uuid-1", "Processing...", Some(111)).unwrap();
        update_agent_status_with_conn(&conn, "uuid-2", "Idle", None).unwrap();
        conn
    }

    #[test]
    fn resolve_by_name_returns_default_fields() {
        let conn = seed();
        let agent = resolve_by_name_or_uuid(&conn, "coder-a1").unwrap();
        assert_eq!(agent.name, "coder-a1");
        assert_eq!(agent.uuid, "uuid-1");
        assert_eq!(agent.class, "Coder");
        assert_eq!(agent.provider, "codex");
        assert_eq!(agent.project, "Wardian");
        assert_eq!(agent.status, "processing");
    }

    #[test]
    fn resolve_by_uuid_returns_peer() {
        let conn = seed();
        let agent = resolve_by_name_or_uuid(&conn, "uuid-2").unwrap();
        assert_eq!(agent.name, "architect-a1");
    }

    #[test]
    fn resolve_self_requires_env() {
        let _guard = crate::tests::env_lock();
        std::env::remove_var("WARDIAN_SESSION_ID");
        let conn = seed();
        assert!(matches!(
            resolve_self(&conn),
            Err(IdentityError::NotInSession)
        ));
    }

    #[test]
    fn resolve_self_uses_session_env() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_SESSION_ID", "uuid-1");
        let conn = seed();
        let agent = resolve_self(&conn).unwrap();
        assert_eq!(agent.uuid, "uuid-1");
        std::env::remove_var("WARDIAN_SESSION_ID");
    }

    #[test]
    fn list_project_scope_uses_caller_project() {
        let conn = seed();
        let agents = list_agents(
            &conn,
            &ListFilters {
                scope: Scope::Project,
                caller_project: Some("Wardian".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn status_filter_normalizes_status_names() {
        let conn = seed();
        let agents = list_agents(
            &conn,
            &ListFilters {
                scope: Scope::All,
                status: Some("processing".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "coder-a1");
    }
}
