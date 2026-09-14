//! Durable identities and lifecycle records for bounded automation workers and
//! provider-spawned children.
//!
//! These records are execution evidence. They deliberately do not reuse the
//! permanent-agent registry and do not make a provider transcript into an
//! editable lifecycle interface.

use crate::native_transport::NativeTransportCapabilities;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::sync::OnceLock;

pub const RESUME_ELIGIBILITY_DAYS: i64 = 7;
pub const DETAIL_RETENTION_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporaryWorkerKind {
    Automation,
    ProviderChild,
}

impl TemporaryWorkerKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Automation => "automation",
            Self::ProviderChild => "provider_child",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "automation" => Ok(Self::Automation),
            "provider_child" => Ok(Self::ProviderChild),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown temporary worker kind: {other}"),
                )
                .into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporaryWorkerState {
    Requested,
    Running,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

impl TemporaryWorkerState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub fn needs_attention(self) -> bool {
        matches!(self, Self::Waiting | Self::Failed | Self::Unknown)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "requested" => Ok(Self::Requested),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "unknown" => Ok(Self::Unknown),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown temporary worker state: {other}"),
                )
                .into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationWorkerOrigin {
    pub blueprint_id: String,
    pub run_id: String,
    pub node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryWorkerCapabilities {
    pub inspection: bool,
    pub follow_up: bool,
    pub interruption: bool,
    pub resume: bool,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeTransportCapabilities>,
}

impl TemporaryWorkerCapabilities {
    pub fn observe_only(reason: impl Into<String>) -> Self {
        Self {
            inspection: true,
            follow_up: false,
            interruption: false,
            resume: false,
            source: reason.into(),
            native: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryWorkerRecord {
    pub worker_id: String,
    pub kind: TemporaryWorkerKind,
    pub provider: String,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_provider_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blueprint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Wardian identity that owns the provider habitat used for this worker.
    pub runtime_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    pub runtime_generation: Option<u64>,
    #[serde(default, skip_serializing)]
    pub owner_instance_id: Option<String>,
    pub state: TemporaryWorkerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub capabilities: TemporaryWorkerCapabilities,
    pub coverage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<String>,
    pub last_observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_follow_up_accepted_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumable_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_retained_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RegisterAutomationWorker<'a> {
    pub provider: &'a str,
    pub workspace: &'a str,
    pub runtime_session_id: &'a str,
    pub origin: &'a AutomationWorkerOrigin,
}

#[derive(Debug, Clone)]
pub struct RegisterProviderChild<'a> {
    pub provider: &'a str,
    pub workspace: &'a str,
    pub root_agent_id: Option<&'a str>,
    pub parent_worker_id: Option<&'a str>,
    pub parent_provider_session_id: &'a str,
    pub runtime_session_id: &'a str,
    pub provider_session_id: &'a str,
    pub automation_origin: Option<&'a AutomationWorkerOrigin>,
    pub state: TemporaryWorkerState,
    pub outcome: Option<&'a str>,
    pub source_path: &'a str,
    pub coverage: &'a str,
    pub requested_at: Option<&'a str>,
    pub terminal_at: Option<&'a str>,
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS temporary_workers (
            worker_id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            provider TEXT NOT NULL,
            workspace TEXT NOT NULL,
            root_agent_id TEXT,
            parent_worker_id TEXT,
            parent_provider_session_id TEXT,
            blueprint_id TEXT,
            run_id TEXT,
            node_id TEXT,
            attempt INTEGER,
            runtime_session_id TEXT NOT NULL,
            provider_session_id TEXT,
            runtime_generation INTEGER,
            state TEXT NOT NULL,
            outcome TEXT,
            capabilities_json TEXT NOT NULL,
            coverage TEXT NOT NULL,
            source_key TEXT,
            source_path TEXT,
            requested_at TEXT NOT NULL,
            started_at TEXT,
            terminal_at TEXT,
            last_observed_at TEXT NOT NULL,
            last_follow_up_accepted_at TEXT,
            resumable_until TEXT,
            detail_retained_until TEXT,
            error TEXT,
            owner_instance_id TEXT,
            FOREIGN KEY(parent_worker_id) REFERENCES temporary_workers(worker_id)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_temporary_worker_automation_attempt
          ON temporary_workers(blueprint_id, run_id, node_id, attempt)
          WHERE kind = 'automation';
        CREATE UNIQUE INDEX IF NOT EXISTS idx_temporary_worker_provider_child
          ON temporary_workers(provider, provider_session_id)
          WHERE kind = 'provider_child' AND provider_session_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_temporary_worker_run
          ON temporary_workers(blueprint_id, run_id, node_id, attempt);
        CREATE INDEX IF NOT EXISTS idx_temporary_worker_root_agent
          ON temporary_workers(root_agent_id, state, last_observed_at);",
    )
}

pub fn register_automation_worker(
    input: RegisterAutomationWorker<'_>,
) -> Result<TemporaryWorkerRecord, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let attempt: u32 = tx.query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM temporary_workers
             WHERE kind = 'automation' AND blueprint_id = ?1 AND run_id = ?2 AND node_id = ?3",
            params![
                input.origin.blueprint_id,
                input.origin.run_id,
                input.origin.node_id
            ],
            |row| row.get(0),
        )?;
        let now = now();
        let record = TemporaryWorkerRecord {
            worker_id: uuid::Uuid::new_v4().to_string(),
            kind: TemporaryWorkerKind::Automation,
            provider: input.provider.to_string(),
            workspace: input.workspace.to_string(),
            root_agent_id: None,
            parent_worker_id: None,
            parent_provider_session_id: None,
            blueprint_id: Some(input.origin.blueprint_id.clone()),
            run_id: Some(input.origin.run_id.clone()),
            node_id: Some(input.origin.node_id.clone()),
            attempt: Some(attempt),
            runtime_session_id: input.runtime_session_id.to_string(),
            provider_session_id: None,
            runtime_generation: Some(u64::from(attempt)),
            owner_instance_id: Some(process_owner_id().to_string()),
            state: TemporaryWorkerState::Requested,
            outcome: None,
            capabilities: TemporaryWorkerCapabilities::observe_only(
                "headless provider capability is not generation-bound",
            ),
            coverage: "pending".to_string(),
            source_key: None,
            source_path: None,
            requested_at: now.clone(),
            started_at: None,
            terminal_at: None,
            last_observed_at: now,
            last_follow_up_accepted_at: None,
            resumable_until: None,
            detail_retained_until: None,
            error: None,
        };
        insert_record(&tx, &record)?;
        tx.commit()?;
        Ok(record)
    })
}

pub fn mark_running(worker: &TemporaryWorkerRecord) -> Result<bool, Box<dyn std::error::Error>> {
    let observed = now();
    crate::db::get_db_conn(|conn| Ok(mark_running_with_conn(conn, worker, &observed)?))
}

fn mark_running_with_conn(
    conn: &Connection,
    worker: &TemporaryWorkerRecord,
    observed: &str,
) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE temporary_workers SET state = 'running', started_at = COALESCE(started_at, ?2),
             last_observed_at = ?2 WHERE worker_id = ?1 AND state = 'requested'
             AND owner_instance_id = ?3 AND runtime_generation = ?4",
        params![
            worker.worker_id,
            observed,
            worker.owner_instance_id,
            worker.runtime_generation,
        ],
    )?;
    Ok(changed == 1)
}

pub fn mark_unknown(
    worker: &TemporaryWorkerRecord,
    coverage: &str,
    error: Option<&str>,
) -> Result<bool, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        Ok(mark_unknown_with_conn(
            conn,
            worker,
            coverage,
            error,
            &now(),
        )?)
    })
}

fn mark_unknown_with_conn(
    conn: &Connection,
    worker: &TemporaryWorkerRecord,
    coverage: &str,
    error: Option<&str>,
    observed: &str,
) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE temporary_workers SET state = 'unknown', coverage = ?2,
             error = COALESCE(?3, error), last_observed_at = ?4 WHERE worker_id = ?1
             AND owner_instance_id = ?5 AND runtime_generation = ?6
             AND state IN ('requested', 'running')",
        params![
            worker.worker_id,
            coverage,
            error,
            observed,
            worker.owner_instance_id,
            worker.runtime_generation,
        ],
    )?;
    Ok(changed == 1)
}

pub fn mark_terminal(
    worker: &TemporaryWorkerRecord,
    state: TemporaryWorkerState,
    outcome: Option<&str>,
    provider_session_id: Option<&str>,
    source_path: Option<&str>,
    coverage: &str,
    error: Option<&str>,
) -> Result<bool, Box<dyn std::error::Error>> {
    if !state.is_terminal() {
        return Err("temporary worker terminal update requires a terminal state".into());
    }
    let observed = Utc::now();
    crate::db::get_db_conn(|conn| {
        Ok(mark_terminal_with_conn(
            conn,
            worker,
            TerminalUpdate {
                state,
                outcome,
                provider_session_id,
                source_path,
                coverage,
                error,
                observed,
            },
        )?)
    })
}

struct TerminalUpdate<'a> {
    state: TemporaryWorkerState,
    outcome: Option<&'a str>,
    provider_session_id: Option<&'a str>,
    source_path: Option<&'a str>,
    coverage: &'a str,
    error: Option<&'a str>,
    observed: DateTime<Utc>,
}

fn mark_terminal_with_conn(
    conn: &Connection,
    worker: &TemporaryWorkerRecord,
    update: TerminalUpdate<'_>,
) -> rusqlite::Result<bool> {
    let TerminalUpdate {
        state,
        outcome,
        provider_session_id,
        source_path,
        coverage,
        error,
        observed,
    } = update;
    let observed_at = observed.to_rfc3339();
    let resumable_until = (observed + Duration::days(RESUME_ELIGIBILITY_DAYS)).to_rfc3339();
    let detail_retained_until = (observed + Duration::days(DETAIL_RETENTION_DAYS)).to_rfc3339();
    let source_key = source_path.map(|path| {
        crate::telemetry::identity::source_key(&worker.provider, &worker.worker_id, path)
    });
    let changed = conn.execute(
        "UPDATE temporary_workers SET state = ?2, outcome = ?3,
             provider_session_id = COALESCE(?4, provider_session_id),
             source_key = COALESCE(?5, source_key), source_path = COALESCE(?6, source_path),
             terminal_at = ?7, last_observed_at = ?7, resumable_until = ?8,
             detail_retained_until = ?9, coverage = ?10, error = ?11
             WHERE worker_id = ?1 AND owner_instance_id = ?12
             AND runtime_generation = ?13 AND state IN ('requested', 'running')",
        params![
            worker.worker_id,
            state.as_str(),
            outcome,
            provider_session_id,
            source_key,
            source_path,
            observed_at,
            resumable_until,
            detail_retained_until,
            coverage,
            error,
            worker.owner_instance_id,
            worker.runtime_generation,
        ],
    )?;
    Ok(changed == 1)
}

pub fn attach_verified_source(
    worker_id: &str,
    source_path: &str,
    coverage: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        let provider = conn
            .query_row(
                "SELECT provider FROM temporary_workers WHERE worker_id = ?1",
                params![worker_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(provider) = provider else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("temporary worker not found: {worker_id}"),
            )
            .into());
        };
        let source_key = crate::telemetry::identity::source_key(&provider, worker_id, source_path);
        conn.execute(
            "UPDATE temporary_workers SET source_key = ?2, source_path = ?3,
             coverage = ?4, last_observed_at = ?5 WHERE worker_id = ?1",
            params![worker_id, source_key, source_path, coverage, now()],
        )?;
        Ok(())
    })
}

/// Move retention windows only after a provider adapter has explicitly
/// accepted a correlated follow-up. Attempted or uncertain delivery does not
/// extend eligibility.
pub fn record_follow_up_accepted(worker_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let accepted = Utc::now();
    crate::db::get_db_conn(|conn| {
        let changed = conn.execute(
            "UPDATE temporary_workers SET last_follow_up_accepted_at = ?2,
             last_observed_at = ?2, resumable_until = ?3, detail_retained_until = ?4
             WHERE worker_id = ?1",
            params![
                worker_id,
                accepted.to_rfc3339(),
                (accepted + Duration::days(RESUME_ELIGIBILITY_DAYS)).to_rfc3339(),
                (accepted + Duration::days(DETAIL_RETENTION_DAYS)).to_rfc3339(),
            ],
        )?;
        if changed == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("temporary worker not found: {worker_id}"),
            )
            .into());
        }
        Ok(())
    })
}

pub fn register_provider_child(
    input: RegisterProviderChild<'_>,
) -> Result<TemporaryWorkerRecord, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        if let Some(existing) =
            find_by_provider_session_with_conn(conn, input.provider, input.provider_session_id)?
        {
            let expected_origin = input.automation_origin.map(|origin| {
                (
                    origin.blueprint_id.as_str(),
                    origin.run_id.as_str(),
                    origin.node_id.as_str(),
                )
            });
            let existing_origin = existing
                .blueprint_id
                .as_deref()
                .zip(existing.run_id.as_deref())
                .zip(existing.node_id.as_deref())
                .map(|((blueprint_id, run_id), node_id)| (blueprint_id, run_id, node_id));
            if existing.root_agent_id.as_deref() != input.root_agent_id
                || existing.parent_provider_session_id.as_deref()
                    != Some(input.parent_provider_session_id)
                || existing_origin != expected_origin
                || existing.parent_worker_id.as_deref().is_some_and(|parent| {
                    input
                        .parent_worker_id
                        .is_some_and(|expected| parent != expected)
                })
            {
                return Err(format!(
                    "conflicting verified ownership for {} session {}",
                    input.provider, input.provider_session_id
                )
                .into());
            }
            let observed = now();
            let retention_baseline = input
                .terminal_at
                .filter(|value| DateTime::parse_from_rfc3339(value).is_ok())
                .unwrap_or(observed.as_str());
            let (terminal_at, resumable_until, detail_retained_until) =
                retention_dates(input.state, retention_baseline);
            conn.execute(
                "UPDATE temporary_workers SET
                 state = CASE
                   WHEN ?2 = 'unknown' AND state IN ('succeeded', 'failed', 'cancelled') THEN state
                   ELSE ?2
                 END,
                 outcome = COALESCE(?3, outcome),
                 parent_worker_id = COALESCE(?4, parent_worker_id), source_path = ?5,
                 source_key = ?6, coverage = ?7, last_observed_at = ?8,
                 terminal_at = COALESCE(terminal_at, ?9),
                 resumable_until = COALESCE(resumable_until, ?10),
                 detail_retained_until = COALESCE(detail_retained_until, ?11)
                 WHERE worker_id = ?1",
                params![
                    existing.worker_id,
                    input.state.as_str(),
                    input.outcome,
                    input.parent_worker_id,
                    input.source_path,
                    crate::telemetry::identity::source_key(
                        input.provider,
                        &existing.worker_id,
                        input.source_path,
                    ),
                    input.coverage,
                    observed,
                    terminal_at,
                    resumable_until,
                    detail_retained_until,
                ],
            )?;
            return load_with_conn(conn, &existing.worker_id)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "updated worker disappeared").into()
            });
        }

        let observed = now();
        let requested_at = input
            .requested_at
            .filter(|value| DateTime::parse_from_rfc3339(value).is_ok())
            .map(str::to_string)
            .unwrap_or_else(|| observed.clone());
        let retention_baseline = input
            .terminal_at
            .filter(|value| DateTime::parse_from_rfc3339(value).is_ok())
            .unwrap_or(observed.as_str());
        let (terminal_at, resumable_until, detail_retained_until) =
            retention_dates(input.state, retention_baseline);
        let worker_id = uuid::Uuid::new_v4().to_string();
        let source_key =
            crate::telemetry::identity::source_key(input.provider, &worker_id, input.source_path);
        let record = TemporaryWorkerRecord {
            worker_id,
            kind: TemporaryWorkerKind::ProviderChild,
            provider: input.provider.to_string(),
            workspace: input.workspace.to_string(),
            root_agent_id: input.root_agent_id.map(str::to_string),
            parent_worker_id: input.parent_worker_id.map(str::to_string),
            parent_provider_session_id: Some(input.parent_provider_session_id.to_string()),
            blueprint_id: input
                .automation_origin
                .map(|origin| origin.blueprint_id.clone()),
            run_id: input.automation_origin.map(|origin| origin.run_id.clone()),
            node_id: input.automation_origin.map(|origin| origin.node_id.clone()),
            attempt: None,
            runtime_session_id: input.runtime_session_id.to_string(),
            provider_session_id: Some(input.provider_session_id.to_string()),
            runtime_generation: None,
            owner_instance_id: None,
            state: input.state,
            outcome: input.outcome.map(str::to_string),
            capabilities: TemporaryWorkerCapabilities::observe_only(
                "codex child adapter is observe-only",
            ),
            coverage: input.coverage.to_string(),
            source_key: Some(source_key),
            source_path: Some(input.source_path.to_string()),
            requested_at: requested_at.clone(),
            started_at: Some(requested_at),
            terminal_at,
            last_observed_at: observed,
            last_follow_up_accepted_at: None,
            resumable_until,
            detail_retained_until,
            error: None,
        };
        insert_record(conn, &record)?;
        Ok(record)
    })
}

pub fn load(worker_id: &str) -> Result<Option<TemporaryWorkerRecord>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| Ok(load_with_conn(conn, worker_id)?))
}

pub fn list_for_run(
    blueprint_id: &str,
    run_id: &str,
) -> Result<Vec<TemporaryWorkerRecord>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        let mut statement = conn.prepare(
            "SELECT worker_id, kind, provider, workspace, root_agent_id, parent_worker_id,
             parent_provider_session_id, blueprint_id, run_id, node_id, attempt,
             runtime_session_id, provider_session_id, runtime_generation, state, outcome,
             capabilities_json, coverage, source_key, source_path, requested_at, started_at,
             terminal_at, last_observed_at, last_follow_up_accepted_at, resumable_until,
             detail_retained_until, error, owner_instance_id FROM temporary_workers
             WHERE blueprint_id = ?1 AND run_id = ?2
             ORDER BY node_id, CASE kind WHEN 'automation' THEN 0 ELSE 1 END,
             attempt, requested_at",
        )?;
        let records = statement
            .query_map(params![blueprint_id, run_id], record_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    })
}

pub fn list_for_root(
    root_agent_id: &str,
) -> Result<Vec<TemporaryWorkerRecord>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| Ok(list_for_root_with_conn(conn, root_agent_id, &now())?))
}

fn list_for_root_with_conn(
    conn: &Connection,
    root_agent_id: &str,
    observed_at: &str,
) -> rusqlite::Result<Vec<TemporaryWorkerRecord>> {
    let mut statement = conn.prepare(
        "SELECT worker_id, kind, provider, workspace, root_agent_id, parent_worker_id,
             parent_provider_session_id, blueprint_id, run_id, node_id, attempt,
             runtime_session_id, provider_session_id, runtime_generation, state, outcome,
             capabilities_json, coverage, source_key, source_path, requested_at, started_at,
             terminal_at, last_observed_at, last_follow_up_accepted_at, resumable_until,
             detail_retained_until, error, owner_instance_id FROM temporary_workers
             WHERE root_agent_id = ?1 AND (
               state IN ('requested', 'running', 'waiting', 'unknown') OR detail_retained_until > ?2
             )
             ORDER BY requested_at, worker_id",
    )?;
    let records = statement
        .query_map(params![root_agent_id, observed_at], record_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(records)
}

pub fn attention_count_for_run(
    blueprint_id: &str,
    run_id: &str,
) -> Result<u32, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM temporary_workers WHERE blueprint_id = ?1 AND run_id = ?2
             AND (state IN ('waiting', 'unknown') OR (state = 'failed' AND detail_retained_until > ?3))",
            params![blueprint_id, run_id, now()],
            |row| row.get(0),
        )?)
    })
}

pub fn attention_counts_by_run(
) -> Result<HashMap<(String, String), u32>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        let mut statement = conn.prepare(
            "SELECT blueprint_id, run_id, COUNT(*) FROM temporary_workers
             WHERE blueprint_id IS NOT NULL AND run_id IS NOT NULL
             AND (state IN ('waiting', 'unknown') OR (state = 'failed' AND detail_retained_until > ?1))
             GROUP BY blueprint_id, run_id",
        )?;
        let counts = statement
            .query_map(params![now()], |row| {
                Ok(((row.get(0)?, row.get(1)?), row.get(2)?))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        Ok(counts)
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootAgentWorkerSummary {
    pub root_agent_id: String,
    pub active: u32,
    pub past: u32,
    pub unknown: u32,
    pub attention_count: u32,
    pub attention_waiting: u32,
    pub attention_failed: u32,
    pub attention_unknown: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryWorkerTelemetry {
    pub worker_id: String,
    pub turns: u32,
    pub tokens: crate::telemetry::TokenCounts,
    pub models: Vec<String>,
    pub efforts: Vec<String>,
}

pub fn telemetry_for_run(
    blueprint_id: &str,
    run_id: &str,
) -> Result<HashMap<String, TemporaryWorkerTelemetry>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        let mut statement = conn.prepare(
            "SELECT t.session_id, COUNT(*), SUM(t.input_tokens),
             SUM(t.cached_input_tokens), SUM(t.cache_write_tokens),
             SUM(t.output_tokens), SUM(t.reasoning_tokens),
             GROUP_CONCAT(DISTINCT t.model), GROUP_CONCAT(DISTINCT t.effort)
             FROM telemetry_turns t
             JOIN temporary_workers w ON w.worker_id = t.session_id
             WHERE w.blueprint_id = ?1 AND w.run_id = ?2
             GROUP BY t.session_id",
        )?;
        let rows = statement.query_map(params![blueprint_id, run_id], |row| {
            let worker_id: String = row.get(0)?;
            Ok((
                worker_id.clone(),
                TemporaryWorkerTelemetry {
                    worker_id,
                    turns: row.get(1)?,
                    tokens: crate::telemetry::TokenCounts {
                        input_tokens: row.get(2)?,
                        cached_input_tokens: row.get(3)?,
                        cache_write_tokens: row.get(4)?,
                        output_tokens: row.get(5)?,
                        reasoning_tokens: row.get(6)?,
                    },
                    models: split_sql_list(row.get(7)?),
                    efforts: split_sql_list(row.get(8)?),
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    })
}

pub fn telemetry_for_root(
    root_agent_id: &str,
) -> Result<HashMap<String, TemporaryWorkerTelemetry>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        let mut statement = conn.prepare(
            "SELECT t.session_id, COUNT(*), SUM(t.input_tokens),
             SUM(t.cached_input_tokens), SUM(t.cache_write_tokens),
             SUM(t.output_tokens), SUM(t.reasoning_tokens),
             GROUP_CONCAT(DISTINCT t.model), GROUP_CONCAT(DISTINCT t.effort)
             FROM telemetry_turns t
             JOIN temporary_workers w ON w.worker_id = t.session_id
             WHERE w.root_agent_id = ?1 AND (
               w.state IN ('requested', 'running', 'waiting', 'unknown')
               OR w.detail_retained_until > ?2
             )
             GROUP BY t.session_id",
        )?;
        let rows = statement.query_map(params![root_agent_id, now()], telemetry_from_row)?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    })
}

fn telemetry_from_row(row: &Row<'_>) -> rusqlite::Result<(String, TemporaryWorkerTelemetry)> {
    let worker_id: String = row.get(0)?;
    Ok((
        worker_id.clone(),
        TemporaryWorkerTelemetry {
            worker_id,
            turns: row.get(1)?,
            tokens: crate::telemetry::TokenCounts {
                input_tokens: row.get(2)?,
                cached_input_tokens: row.get(3)?,
                cache_write_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                reasoning_tokens: row.get(6)?,
            },
            models: split_sql_list(row.get(7)?),
            efforts: split_sql_list(row.get(8)?),
        },
    ))
}

fn split_sql_list(value: Option<String>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn root_agent_summaries() -> Result<Vec<RootAgentWorkerSummary>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| Ok(root_agent_summaries_with_conn(conn, &now())?))
}

fn root_agent_summaries_with_conn(
    conn: &Connection,
    observed_at: &str,
) -> rusqlite::Result<Vec<RootAgentWorkerSummary>> {
    let mut statement = conn.prepare(
        "SELECT root_agent_id,
         SUM(CASE WHEN state IN ('requested', 'running', 'waiting') THEN 1 ELSE 0 END),
         SUM(CASE WHEN state IN ('succeeded', 'failed', 'cancelled') THEN 1 ELSE 0 END),
         SUM(CASE WHEN state = 'unknown' THEN 1 ELSE 0 END),
         SUM(CASE WHEN state IN ('waiting', 'failed', 'unknown') THEN 1 ELSE 0 END),
         SUM(CASE WHEN state = 'waiting' THEN 1 ELSE 0 END),
         SUM(CASE WHEN state = 'failed' THEN 1 ELSE 0 END),
         SUM(CASE WHEN state = 'unknown' THEN 1 ELSE 0 END)
         FROM temporary_workers WHERE root_agent_id IS NOT NULL AND (
           state IN ('requested', 'running', 'waiting', 'unknown') OR detail_retained_until > ?1
         )
         GROUP BY root_agent_id ORDER BY root_agent_id",
    )?;
    let summaries = statement
        .query_map(params![observed_at], |row| {
            Ok(RootAgentWorkerSummary {
                root_agent_id: row.get(0)?,
                active: row.get(1)?,
                past: row.get(2)?,
                unknown: row.get(3)?,
                attention_count: row.get(4)?,
                attention_waiting: row.get(5)?,
                attention_failed: row.get(6)?,
                attention_unknown: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(summaries)
}

pub fn telemetry_records() -> Result<Vec<TemporaryWorkerRecord>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        let now = now();
        let mut statement = conn.prepare(
            "SELECT worker_id, kind, provider, workspace, root_agent_id, parent_worker_id,
             parent_provider_session_id, blueprint_id, run_id, node_id, attempt,
             runtime_session_id, provider_session_id, runtime_generation, state, outcome,
             capabilities_json, coverage, source_key, source_path, requested_at, started_at,
             terminal_at, last_observed_at, last_follow_up_accepted_at, resumable_until,
             detail_retained_until, error, owner_instance_id FROM temporary_workers
             WHERE source_path IS NOT NULL AND (
               state IN ('requested', 'running', 'waiting', 'unknown') OR detail_retained_until > ?1
             )",
        )?;
        let records = statement
            .query_map(params![now], record_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    })
}

pub fn codex_automation_roots() -> Result<Vec<TemporaryWorkerRecord>, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        let now = now();
        let mut statement = conn.prepare(
            "SELECT worker_id, kind, provider, workspace, root_agent_id, parent_worker_id,
             parent_provider_session_id, blueprint_id, run_id, node_id, attempt,
             runtime_session_id, provider_session_id, runtime_generation, state, outcome,
             capabilities_json, coverage, source_key, source_path, requested_at, started_at,
             terminal_at, last_observed_at, last_follow_up_accepted_at, resumable_until,
             detail_retained_until, error, owner_instance_id FROM temporary_workers
             WHERE kind = 'automation' AND provider = 'codex'
             AND provider_session_id IS NOT NULL AND (
               state IN ('requested', 'running', 'waiting', 'unknown') OR detail_retained_until > ?1
             )",
        )?;
        let records = statement
            .query_map(params![now], record_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    })
}

/// A provider process is never kept alive for retention. On a new app process,
/// active automation records from the previous owner become unknown until
/// provider evidence resolves them.
pub fn reconcile_stale_automation_owners() -> Result<usize, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| {
        Ok(reconcile_stale_automation_owners_with_conn(
            conn,
            process_owner_id(),
            &now(),
        )?)
    })
}

fn reconcile_stale_automation_owners_with_conn(
    conn: &Connection,
    current_instance_id: &str,
    observed_at: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE temporary_workers SET state = 'unknown', last_observed_at = ?2,
         coverage = 'runtime_owner_lost'
         WHERE kind = 'automation' AND state IN ('requested', 'running')
         AND owner_instance_id IS NOT NULL AND owner_instance_id != ?1",
        params![current_instance_id, observed_at],
    )
}

/// Drop registry-owned detail references after the bounded retention window.
/// Provider transcripts, telemetry facts, usage evidence, and user resources
/// live in their own stores and are never deleted here.
pub fn apply_detail_retention() -> Result<usize, Box<dyn std::error::Error>> {
    crate::db::get_db_conn(|conn| Ok(apply_detail_retention_with_conn(conn, &now())?))
}

fn apply_detail_retention_with_conn(
    conn: &Connection,
    observed_at: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE temporary_workers SET outcome = NULL, error = NULL,
         source_key = NULL, source_path = NULL, coverage = 'detail_retention_expired'
         WHERE state IN ('succeeded', 'failed', 'cancelled')
         AND detail_retained_until IS NOT NULL AND detail_retained_until <= ?1
         AND (source_path IS NOT NULL OR outcome IS NOT NULL OR error IS NOT NULL)",
        params![observed_at],
    )
}

fn insert_record(conn: &Connection, record: &TemporaryWorkerRecord) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO temporary_workers (
          worker_id, kind, provider, workspace, root_agent_id, parent_worker_id,
          parent_provider_session_id, blueprint_id, run_id, node_id, attempt,
          runtime_session_id, provider_session_id, runtime_generation, state, outcome,
          capabilities_json, coverage, source_key, source_path, requested_at, started_at,
          terminal_at, last_observed_at, last_follow_up_accepted_at, resumable_until,
          detail_retained_until, error, owner_instance_id
        ) VALUES (
          ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
          ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29
        )",
        params![
            record.worker_id,
            record.kind.as_str(),
            record.provider,
            record.workspace,
            record.root_agent_id,
            record.parent_worker_id,
            record.parent_provider_session_id,
            record.blueprint_id,
            record.run_id,
            record.node_id,
            record.attempt,
            record.runtime_session_id,
            record.provider_session_id,
            record.runtime_generation,
            record.state.as_str(),
            record.outcome,
            serde_json::to_string(&record.capabilities).map_err(to_sql_error)?,
            record.coverage,
            record.source_key,
            record.source_path,
            record.requested_at,
            record.started_at,
            record.terminal_at,
            record.last_observed_at,
            record.last_follow_up_accepted_at,
            record.resumable_until,
            record.detail_retained_until,
            record.error,
            record.owner_instance_id,
        ],
    )?;
    Ok(())
}

fn find_by_provider_session_with_conn(
    conn: &Connection,
    provider: &str,
    provider_session_id: &str,
) -> rusqlite::Result<Option<TemporaryWorkerRecord>> {
    conn.query_row(
        "SELECT worker_id, kind, provider, workspace, root_agent_id, parent_worker_id,
         parent_provider_session_id, blueprint_id, run_id, node_id, attempt,
         runtime_session_id, provider_session_id, runtime_generation, state, outcome,
         capabilities_json, coverage, source_key, source_path, requested_at, started_at,
         terminal_at, last_observed_at, last_follow_up_accepted_at, resumable_until,
         detail_retained_until, error, owner_instance_id FROM temporary_workers
         WHERE provider = ?1 AND provider_session_id = ?2",
        params![provider, provider_session_id],
        record_from_row,
    )
    .optional()
}

fn load_with_conn(
    conn: &Connection,
    worker_id: &str,
) -> rusqlite::Result<Option<TemporaryWorkerRecord>> {
    conn.query_row(
        "SELECT worker_id, kind, provider, workspace, root_agent_id, parent_worker_id,
         parent_provider_session_id, blueprint_id, run_id, node_id, attempt,
         runtime_session_id, provider_session_id, runtime_generation, state, outcome,
         capabilities_json, coverage, source_key, source_path, requested_at, started_at,
         terminal_at, last_observed_at, last_follow_up_accepted_at, resumable_until,
         detail_retained_until, error, owner_instance_id FROM temporary_workers WHERE worker_id = ?1",
        params![worker_id],
        record_from_row,
    )
    .optional()
}

fn record_from_row(row: &Row<'_>) -> rusqlite::Result<TemporaryWorkerRecord> {
    let capabilities_json: String = row.get(16)?;
    Ok(TemporaryWorkerRecord {
        worker_id: row.get(0)?,
        kind: TemporaryWorkerKind::parse(&row.get::<_, String>(1)?)?,
        provider: row.get(2)?,
        workspace: row.get(3)?,
        root_agent_id: row.get(4)?,
        parent_worker_id: row.get(5)?,
        parent_provider_session_id: row.get(6)?,
        blueprint_id: row.get(7)?,
        run_id: row.get(8)?,
        node_id: row.get(9)?,
        attempt: row.get(10)?,
        runtime_session_id: row.get(11)?,
        provider_session_id: row.get(12)?,
        runtime_generation: row.get(13)?,
        state: TemporaryWorkerState::parse(&row.get::<_, String>(14)?)?,
        outcome: row.get(15)?,
        capabilities: serde_json::from_str(&capabilities_json).map_err(from_sql_error)?,
        coverage: row.get(17)?,
        source_key: row.get(18)?,
        source_path: row.get(19)?,
        requested_at: row.get(20)?,
        started_at: row.get(21)?,
        terminal_at: row.get(22)?,
        last_observed_at: row.get(23)?,
        last_follow_up_accepted_at: row.get(24)?,
        resumable_until: row.get(25)?,
        detail_retained_until: row.get(26)?,
        error: row.get(27)?,
        owner_instance_id: row.get(28)?,
    })
}

fn retention_dates(
    state: TemporaryWorkerState,
    observed_at: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    if !state.is_terminal() {
        return (None, None, None);
    }
    let observed = DateTime::parse_from_rfc3339(observed_at)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    (
        Some(observed.to_rfc3339()),
        Some((observed + Duration::days(RESUME_ELIGIBILITY_DAYS)).to_rfc3339()),
        Some((observed + Duration::days(DETAIL_RETENTION_DAYS)).to_rfc3339()),
    )
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn process_owner_id() -> &'static str {
    static OWNER: OnceLock<String> = OnceLock::new();
    OWNER.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

fn to_sql_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(error.into())
}

fn from_sql_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(worker_id: &str, attempt: u32) -> TemporaryWorkerRecord {
        TemporaryWorkerRecord {
            worker_id: worker_id.to_string(),
            kind: TemporaryWorkerKind::Automation,
            provider: "codex".into(),
            workspace: "/workspace".into(),
            root_agent_id: None,
            parent_worker_id: None,
            parent_provider_session_id: None,
            blueprint_id: Some("flow".into()),
            run_id: Some("run-1".into()),
            node_id: Some("research".into()),
            attempt: Some(attempt),
            runtime_session_id: "runtime".into(),
            provider_session_id: None,
            runtime_generation: Some(u64::from(attempt)),
            owner_instance_id: Some("owner-a".into()),
            state: TemporaryWorkerState::Requested,
            outcome: None,
            capabilities: TemporaryWorkerCapabilities::observe_only("test"),
            coverage: "pending".into(),
            source_key: None,
            source_path: None,
            requested_at: now(),
            started_at: None,
            terminal_at: None,
            last_observed_at: now(),
            last_follow_up_accepted_at: None,
            resumable_until: None,
            detail_retained_until: None,
            error: None,
        }
    }

    #[test]
    fn migration_allocates_distinct_attempts_and_preserves_structured_origin() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let origin = AutomationWorkerOrigin {
            blueprint_id: "flow".into(),
            run_id: "run-1".into(),
            node_id: "research".into(),
        };

        let insert = |conn: &Connection| {
            let tx = conn.unchecked_transaction().unwrap();
            let attempt: u32 = tx
                .query_row(
                    "SELECT COALESCE(MAX(attempt), 0) + 1 FROM temporary_workers
                     WHERE kind = 'automation' AND blueprint_id = ?1 AND run_id = ?2 AND node_id = ?3",
                    params![origin.blueprint_id, origin.run_id, origin.node_id],
                    |row| row.get(0),
                )
                .unwrap();
            let record = sample_record(&uuid::Uuid::new_v4().to_string(), attempt);
            insert_record(&tx, &record).unwrap();
            tx.commit().unwrap();
            record
        };

        assert_eq!(insert(&conn).attempt, Some(1));
        assert_eq!(insert(&conn).attempt, Some(2));
    }

    #[test]
    fn unknown_workers_have_no_retention_expiry() {
        assert_eq!(
            retention_dates(TemporaryWorkerState::Unknown, "2026-09-13T00:00:00Z"),
            (None, None, None)
        );
        let (_, resumable, detail) =
            retention_dates(TemporaryWorkerState::Succeeded, "2026-09-13T00:00:00Z");
        assert_eq!(resumable.as_deref(), Some("2026-09-20T00:00:00+00:00"));
        assert_eq!(detail.as_deref(), Some("2026-10-13T00:00:00+00:00"));
    }

    #[test]
    fn restart_reconciliation_marks_only_previous_owner_active_workers_unknown() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut stale = sample_record("stale", 1);
        stale.state = TemporaryWorkerState::Running;
        stale.owner_instance_id = Some("previous-process".into());
        insert_record(&conn, &stale).unwrap();
        let mut current = sample_record("current", 2);
        current.state = TemporaryWorkerState::Running;
        current.owner_instance_id = Some("current-process".into());
        insert_record(&conn, &current).unwrap();

        assert_eq!(
            reconcile_stale_automation_owners_with_conn(
                &conn,
                "current-process",
                "2026-09-13T01:00:00Z"
            )
            .unwrap(),
            1
        );
        assert_eq!(
            load_with_conn(&conn, "stale").unwrap().unwrap().state,
            TemporaryWorkerState::Unknown
        );
        assert_eq!(
            load_with_conn(&conn, "current").unwrap().unwrap().state,
            TemporaryWorkerState::Running
        );
    }

    #[test]
    fn lifecycle_updates_require_matching_owner_generation_and_active_state() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut current = sample_record("guarded", 3);
        current.state = TemporaryWorkerState::Running;
        insert_record(&conn, &current).unwrap();

        let mut wrong_generation = current.clone();
        wrong_generation.runtime_generation = Some(4);
        assert!(!mark_unknown_with_conn(
            &conn,
            &wrong_generation,
            "stale_future",
            None,
            "2026-09-13T01:00:00Z",
        )
        .unwrap());
        assert_eq!(
            load_with_conn(&conn, "guarded").unwrap().unwrap().state,
            TemporaryWorkerState::Running
        );

        assert!(mark_terminal_with_conn(
            &conn,
            &current,
            TerminalUpdate {
                state: TemporaryWorkerState::Cancelled,
                outcome: Some("cancelled_by_run"),
                provider_session_id: None,
                source_path: None,
                coverage: "run_cancellation_acknowledged",
                error: None,
                observed: DateTime::parse_from_rfc3339("2026-09-13T01:01:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
        )
        .unwrap());
        assert!(!mark_terminal_with_conn(
            &conn,
            &current,
            TerminalUpdate {
                state: TemporaryWorkerState::Succeeded,
                outcome: Some("completed"),
                provider_session_id: None,
                source_path: None,
                coverage: "provider_session_unavailable",
                error: None,
                observed: DateTime::parse_from_rfc3339("2026-09-13T01:02:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            },
        )
        .unwrap());
        assert_eq!(
            load_with_conn(&conn, "guarded").unwrap().unwrap().state,
            TemporaryWorkerState::Cancelled
        );
    }

    #[test]
    fn stale_future_cannot_overwrite_owner_reconciliation_unknown() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut stale = sample_record("stale-future", 1);
        stale.state = TemporaryWorkerState::Running;
        stale.owner_instance_id = Some("previous-process".into());
        insert_record(&conn, &stale).unwrap();
        reconcile_stale_automation_owners_with_conn(
            &conn,
            "replacement-process",
            "2026-09-13T02:00:00Z",
        )
        .unwrap();

        assert!(!mark_terminal_with_conn(
            &conn,
            &stale,
            TerminalUpdate {
                state: TemporaryWorkerState::Succeeded,
                outcome: Some("completed"),
                provider_session_id: None,
                source_path: None,
                coverage: "provider_session_unavailable",
                error: None,
                observed: Utc::now(),
            },
        )
        .unwrap());
        let reconciled = load_with_conn(&conn, "stale-future").unwrap().unwrap();
        assert_eq!(reconciled.state, TemporaryWorkerState::Unknown);
        assert_eq!(reconciled.coverage, "runtime_owner_lost");
    }

    #[test]
    fn root_agent_summaries_serialize_status_categories_and_attention_reasons() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        for (worker_id, state, detail_retained_until) in [
            ("requested", TemporaryWorkerState::Requested, None),
            ("running", TemporaryWorkerState::Running, None),
            ("waiting", TemporaryWorkerState::Waiting, None),
            (
                "succeeded",
                TemporaryWorkerState::Succeeded,
                Some("2026-10-01T00:00:00Z"),
            ),
            (
                "failed",
                TemporaryWorkerState::Failed,
                Some("2026-10-01T00:00:00Z"),
            ),
            (
                "cancelled",
                TemporaryWorkerState::Cancelled,
                Some("2026-10-01T00:00:00Z"),
            ),
            ("unknown", TemporaryWorkerState::Unknown, None),
            (
                "expired",
                TemporaryWorkerState::Succeeded,
                Some("2026-09-01T00:00:00Z"),
            ),
        ] {
            let mut record = sample_record(worker_id, 1);
            record.kind = TemporaryWorkerKind::ProviderChild;
            record.root_agent_id = Some("root-a".into());
            record.state = state;
            record.detail_retained_until = detail_retained_until.map(str::to_string);
            insert_record(&conn, &record).unwrap();
        }

        let summaries = root_agent_summaries_with_conn(&conn, "2026-09-13T00:00:00Z").unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0],
            RootAgentWorkerSummary {
                root_agent_id: "root-a".into(),
                active: 3,
                past: 3,
                unknown: 1,
                attention_count: 3,
                attention_waiting: 1,
                attention_failed: 1,
                attention_unknown: 1,
            }
        );

        let serialized = serde_json::to_value(&summaries[0]).unwrap();
        assert_eq!(serialized["active"], 3);
        assert_eq!(serialized["past"], 3);
        assert_eq!(serialized["unknown"], 1);
        assert_eq!(serialized["attention_count"], 3);
        assert_eq!(serialized["attention_waiting"], 1);
        assert_eq!(serialized["attention_failed"], 1);
        assert_eq!(serialized["attention_unknown"], 1);
        assert!(serialized.get("total").is_none());
        assert!(serialized.get("attention").is_none());
    }

    #[test]
    fn root_detail_listing_excludes_unrelated_and_expired_children() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut first = sample_record("child-1", 1);
        first.kind = TemporaryWorkerKind::ProviderChild;
        first.root_agent_id = Some("root-a".into());
        first.state = TemporaryWorkerState::Running;
        insert_record(&conn, &first).unwrap();
        let mut retained = sample_record("child-2", 2);
        retained.kind = TemporaryWorkerKind::ProviderChild;
        retained.root_agent_id = Some("root-a".into());
        retained.state = TemporaryWorkerState::Succeeded;
        retained.detail_retained_until = Some("2026-10-01T00:00:00Z".into());
        insert_record(&conn, &retained).unwrap();
        let mut expired = sample_record("child-expired", 3);
        expired.kind = TemporaryWorkerKind::ProviderChild;
        expired.root_agent_id = Some("root-a".into());
        expired.state = TemporaryWorkerState::Succeeded;
        expired.detail_retained_until = Some("2026-09-01T00:00:00Z".into());
        insert_record(&conn, &expired).unwrap();
        let mut unrelated = sample_record("child-other", 4);
        unrelated.kind = TemporaryWorkerKind::ProviderChild;
        unrelated.root_agent_id = Some("root-b".into());
        unrelated.state = TemporaryWorkerState::Running;
        insert_record(&conn, &unrelated).unwrap();

        let details = list_for_root_with_conn(&conn, "root-a", "2026-09-13T00:00:00Z").unwrap();
        assert_eq!(
            details
                .iter()
                .map(|worker| worker.worker_id.as_str())
                .collect::<Vec<_>>(),
            vec!["child-1", "child-2"]
        );
    }

    #[test]
    fn detail_retention_clears_only_registry_owned_detail() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut record = sample_record("completed", 1);
        record.state = TemporaryWorkerState::Succeeded;
        record.outcome = Some("completed".into());
        record.source_key = Some("source".into());
        record.source_path = Some("rollout.jsonl".into());
        record.detail_retained_until = Some("2026-09-12T00:00:00Z".into());
        insert_record(&conn, &record).unwrap();

        assert_eq!(
            apply_detail_retention_with_conn(&conn, "2026-09-13T00:00:00Z").unwrap(),
            1
        );
        let retained = load_with_conn(&conn, "completed").unwrap().unwrap();
        assert_eq!(retained.state, TemporaryWorkerState::Succeeded);
        assert_eq!(retained.blueprint_id.as_deref(), Some("flow"));
        assert!(retained.source_path.is_none());
        assert!(retained.outcome.is_none());
        assert_eq!(retained.coverage, "detail_retention_expired");
    }
}
