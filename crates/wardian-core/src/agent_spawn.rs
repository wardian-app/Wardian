//! Durable admission and ownership records for asynchronous agent spawns.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

mod legacy_names;
mod removal;
pub(crate) use legacy_names::delete_removed_agent_claims;
pub use legacy_names::{
    abort_legacy_rename, begin_legacy_name_effects, complete_legacy_name, finalize_legacy_name,
    recover_legacy_names, reserve_legacy_name, LegacyNameFinalization, LegacyNamePurpose,
};
pub(crate) use removal::reject_bound_ordinary_delete;
pub use removal::{
    admit_bound_removal, bound_removal_ticket, complete_bound_removal, pending_bound_removals,
    requires_bound_removal, seal_bound_removal, BoundRemovalTicket, RemovalAgentIdentity,
    RemovalClosureReceipt, RemovalProfileIdentity,
};

pub const MAX_QUEUED_OPERATIONS: usize = 64;
pub const MAX_OBSERVER_WAIT_MS: u64 = 60_000;

const NAME_SUFFIXES: &[&str] = &[
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliett",
    "kilo", "lima", "mike", "november", "oscar", "papa", "quebec", "romeo", "sierra", "tango",
    "uniform", "victor", "whiskey", "xray", "yankee", "zulu",
];
const MAX_NAME_CHARS: usize = 64;
const MAX_GENERATED_NAME_CYCLES: usize = 100;

/// Typed user intent persisted before any provider or registration side effect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSpawnRequest {
    pub provider: String,
    pub class: String,
    pub name: Option<String>,
    pub workspace: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

impl AgentSpawnRequest {
    pub fn validate_form(&self) -> Result<(), String> {
        for (label, value, limit) in [
            ("provider", self.provider.as_str(), 64),
            ("class", self.class.as_str(), 128),
        ] {
            if value.trim().is_empty() || value.chars().count() > limit {
                return Err(format!("invalid agent spawn {label}"));
            }
        }
        for (label, value, limit) in [
            ("name", self.name.as_deref(), MAX_NAME_CHARS),
            ("workspace", self.workspace.as_deref(), 4096),
            ("model", self.model.as_deref(), 256),
            ("reasoning effort", self.reasoning_effort.as_deref(), 64),
        ] {
            if value.is_some_and(|value| value.chars().count() > limit) {
                return Err(format!("agent spawn {label} exceeds its size limit"));
            }
        }
        if let Some(name) = self.name.as_deref().filter(|name| !name.trim().is_empty()) {
            validate_agent_name(name)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSpawnState {
    Queued,
    Preparing,
    Launching,
    Registered,
    Failed,
    OutcomeUnknown,
}

impl AgentSpawnState {
    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "preparing" => Ok(Self::Preparing),
            "launching" => Ok(Self::Launching),
            "registered" => Ok(Self::Registered),
            "failed" => Ok(Self::Failed),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Registered | Self::Failed | Self::OutcomeUnknown)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSpawnOperation {
    pub operation_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub provider: String,
    pub class: String,
    pub state: AgentSpawnState,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpawnPlan {
    pub operation: AgentSpawnOperation,
    pub request: AgentSpawnRequest,
    pub provider_session_id: Option<String>,
    pub attempt_id: Option<String>,
    pub prepared_workspace: Option<String>,
    pub first_native_session_id: Option<String>,
    pub first_native_published: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub operation: AgentSpawnOperation,
    pub created: bool,
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_spawn_operations (
            operation_id TEXT PRIMARY KEY,
            payload_hash TEXT NOT NULL,
            request_json TEXT NOT NULL,
            planned_agent_id TEXT NOT NULL UNIQUE,
            provider_session_id TEXT,
            session_name TEXT NOT NULL,
            provider TEXT NOT NULL,
            class TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN (
                'queued', 'preparing', 'launching', 'registered', 'failed', 'outcome_unknown'
            )),
            attempt_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            error_code TEXT,
            error_message TEXT,
            launch_context_json TEXT,
            bootstrap_receipt_json TEXT
        )",
        [],
    )?;
    ensure_spawn_column(conn, "launch_context_json", "TEXT")?;
    ensure_spawn_column(conn, "bootstrap_receipt_json", "TEXT")?;
    ensure_spawn_column(conn, "prepared_workspace", "TEXT")?;
    ensure_spawn_column(conn, "first_native_session_id", "TEXT")?;
    ensure_spawn_column(conn, "first_native_generation", "INTEGER")?;
    ensure_spawn_column(conn, "first_native_allowed_generation", "INTEGER")?;
    ensure_spawn_column(conn, "first_native_published", "INTEGER NOT NULL DEFAULT 0")?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_spawn_queue
         ON agent_spawn_operations(state, created_at, operation_id)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_spawn_planned_agent
         ON agent_spawn_operations(planned_agent_id, state)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agent_name_claims (
            session_name TEXT PRIMARY KEY,
            owner_kind TEXT NOT NULL CHECK (owner_kind IN ('operation', 'legacy', 'agent')),
            owner_id TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_name_claim_owner
         ON agent_name_claims(owner_kind, owner_id)",
        [],
    )?;
    legacy_names::migrate(conn)?;
    Ok(())
}

fn ensure_spawn_column(conn: &Connection, name: &str, declaration: &str) -> rusqlite::Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(agent_spawn_operations)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == name) {
        conn.execute_batch(&format!(
            "ALTER TABLE agent_spawn_operations ADD COLUMN {name} {declaration}"
        ))?;
    }
    Ok(())
}

/// Persist an operation and its name claim atomically. Existing keys are
/// compared before queue capacity so retries remain idempotent when saturated.
pub fn admit(
    operation_id: &str,
    request: &AgentSpawnRequest,
    planned_agent_id: &str,
    provider_session_id: Option<&str>,
) -> Result<Admission, String> {
    let operation_id = canonical_uuid(operation_id, "operation id")?;
    let planned_agent_id = canonical_uuid(planned_agent_id, "planned agent id")?;
    request.validate_form()?;
    let request_json = serde_json::to_string(request).map_err(|error| error.to_string())?;
    let payload_hash = format!("{:x}", Sha256::digest(request_json.as_bytes()));
    crate::db::get_db_conn(|conn| {
        admit_with_conn(
            conn,
            &operation_id,
            request,
            &planned_agent_id,
            provider_session_id,
            &request_json,
            &payload_hash,
        )
    })
    .map_err(|error| error.to_string())
}

fn admit_with_conn(
    conn: &Connection,
    operation_id: &str,
    request: &AgentSpawnRequest,
    planned_agent_id: &str,
    provider_session_id: Option<&str>,
    request_json: &str,
    payload_hash: &str,
) -> Result<Admission, Box<dyn std::error::Error>> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if let Some((stored_hash, operation)) = tx
        .query_row(
            "SELECT payload_hash, operation_id FROM agent_spawn_operations
             WHERE operation_id = ?1",
            params![operation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if stored_hash != payload_hash {
            return Err(std::io::Error::other(
                "operation id is already bound to a different spawn request",
            )
            .into());
        }
        let operation = load_operation_with_conn(&tx, &operation)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        tx.commit()?;
        return Ok(Admission {
            operation,
            created: false,
        });
    }

    let queued: i64 = tx.query_row(
        "SELECT COUNT(*) FROM agent_spawn_operations WHERE state = 'queued'",
        [],
        |row| row.get(0),
    )?;
    if queued >= MAX_QUEUED_OPERATIONS as i64 {
        return Err(std::io::Error::other("agent spawn queue is full").into());
    }

    let name = resolve_requested_spawn_name_with_conn(
        &tx,
        request.name.as_deref().unwrap_or_default(),
        &request.class,
    )?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    tx.execute(
        "INSERT INTO agent_name_claims(session_name, owner_kind, owner_id, created_at)
         VALUES (?1, 'operation', ?2, ?3)",
        params![name, operation_id, now],
    )?;
    tx.execute(
        "INSERT INTO agent_spawn_operations (
            operation_id, payload_hash, request_json, planned_agent_id,
            provider_session_id, session_name, provider, class, state,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', ?9, ?9)",
        params![
            operation_id,
            payload_hash,
            request_json,
            planned_agent_id,
            provider_session_id,
            name,
            request.provider,
            request.class,
            now,
        ],
    )?;
    let operation =
        load_operation_with_conn(&tx, operation_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    tx.commit()?;
    Ok(Admission {
        operation,
        created: true,
    })
}

pub fn get(operation_id: &str) -> Result<Option<AgentSpawnOperation>, String> {
    crate::db::get_db_conn(|conn| Ok(load_operation_with_conn(conn, operation_id)?))
        .map_err(|error| error.to_string())
}

pub fn plan(operation_id: &str) -> Result<Option<AgentSpawnPlan>, String> {
    crate::db::get_db_conn(|conn| Ok(load_plan_with_conn(conn, operation_id)?))
        .map_err(|error| error.to_string())
}
pub fn queued_operation_ids() -> Result<Vec<String>, String> {
    crate::db::get_db_conn(|conn| {
        let mut statement = conn.prepare(
            "SELECT operation_id FROM agent_spawn_operations
             WHERE state = 'queued' ORDER BY created_at, operation_id",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .map_err(|error| error.to_string())
}

/// Convert interrupted claims to quarantine and return only never-started work.
pub fn recover_after_owner_acquired() -> Result<Vec<String>, String> {
    crate::db::get_db_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        // Numeric terminal generations restart at each boot. Invalidate every
        // unconsumed old-owner permission before restore can allocate one again.
        tx.execute("UPDATE agent_spawn_operations SET first_native_allowed_generation = NULL
            WHERE first_native_session_id IS NULL", [])?;
        tx.execute(
            "UPDATE agent_spawn_operations
             SET state = 'outcome_unknown', updated_at = ?1,
                 error_code = 'interrupted_after_claim',
                 error_message = 'The prior launch boundary was interrupted; automatic replay is disabled.'
             WHERE state IN ('preparing', 'launching')",
            params![now],
        )?;
        let mut statement = tx.prepare(
            "SELECT operation_id FROM agent_spawn_operations
             WHERE state = 'queued' ORDER BY created_at, operation_id",
        )?;
        let queued = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        drop(statement);
        tx.commit()?;
        Ok(queued)
    })
    .map_err(|error| error.to_string())
}

pub fn claim_queued(operation_id: &str) -> Result<Option<AgentSpawnPlan>, String> {
    let attempt_id = uuid::Uuid::new_v4().to_string();
    crate::db::get_db_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = tx.execute(
            "UPDATE agent_spawn_operations SET state = 'preparing', attempt_id = ?2, updated_at = ?3
             WHERE operation_id = ?1 AND state = 'queued'",
            params![operation_id, attempt_id, now],
        )?;
        if changed != 1 {
            tx.commit()?;
            return Ok(None);
        }
        let plan = load_plan_with_conn(&tx, operation_id)?;
        tx.commit()?;
        Ok(plan)
    })
    .map_err(|error| error.to_string())
}

pub fn mark_launching(operation_id: &str, attempt_id: &str) -> Result<bool, String> {
    update_attempt_state(operation_id, attempt_id, "preparing", "launching")
}

pub fn record_provider_session_id(
    operation_id: &str,
    attempt_id: &str,
    provider_session_id: &str,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = conn.execute(
            "UPDATE agent_spawn_operations SET provider_session_id = ?3, updated_at = ?4
             WHERE operation_id = ?1 AND attempt_id = ?2 AND state = 'launching'",
            params![operation_id, attempt_id, provider_session_id, now],
        )?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

pub fn set_prepared_identity(
    operation_id: &str,
    attempt_id: &str,
    class: &str,
    workspace: &str,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = conn.execute(
            "UPDATE agent_spawn_operations SET class = ?3, prepared_workspace = ?4, updated_at = ?5
             WHERE operation_id = ?1 AND attempt_id = ?2 AND state = 'preparing'",
            params![operation_id, attempt_id, class, workspace, now],
        )?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

pub fn record_launch_context(
    operation_id: &str,
    attempt_id: &str,
    descriptor_json: &str,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = conn.execute(
            "UPDATE agent_spawn_operations SET launch_context_json = ?3, updated_at = ?4
             WHERE operation_id = ?1 AND attempt_id = ?2 AND state = 'preparing'",
            params![operation_id, attempt_id, descriptor_json, now],
        )?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

pub fn record_bootstrap_receipt(
    operation_id: &str,
    attempt_id: &str,
    receipt_json: &str,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = conn.execute(
            "UPDATE agent_spawn_operations SET bootstrap_receipt_json = ?3, updated_at = ?4
             WHERE operation_id = ?1 AND attempt_id = ?2 AND state = 'launching'",
            params![operation_id, attempt_id, receipt_json, now],
        )?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

pub fn fail_before_launch(
    operation_id: &str,
    attempt_id: &str,
    error_code: &str,
    safe_message: &str,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = tx.execute(
            "UPDATE agent_spawn_operations SET state = 'failed', updated_at = ?3,
                 error_code = ?4, error_message = ?5
             WHERE operation_id = ?1 AND attempt_id = ?2 AND state = 'preparing'",
            params![operation_id, attempt_id, now, error_code, safe_message],
        )?;
        if changed == 1 {
            tx.execute(
                "DELETE FROM agent_name_claims WHERE owner_kind = 'operation' AND owner_id = ?1",
                params![operation_id],
            )?;
        }
        tx.commit()?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

pub fn mark_outcome_unknown(operation_id: &str, attempt_id: &str) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = conn.execute(
            "UPDATE agent_spawn_operations SET state = 'outcome_unknown', updated_at = ?3,
                 error_code = 'launch_outcome_unconfirmed',
                 error_message = 'Launch outcome could not be confirmed; automatic replay is disabled.'
             WHERE operation_id = ?1 AND attempt_id = ?2
               AND state IN ('preparing', 'launching')",
            params![operation_id, attempt_id, now],
        )?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

pub fn mark_orphaned_claim_unknown(operation_id: &str) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = conn.execute(
            "UPDATE agent_spawn_operations SET state = 'outcome_unknown', updated_at = ?2,
                 error_code = 'missing_attempt_id',
                 error_message = 'The launch claim is incomplete; automatic replay is disabled.'
             WHERE operation_id = ?1 AND state = 'preparing'",
            params![operation_id, now],
        )?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

/// Commit success only after the normal roster and SQLite summary both name
/// the exact planned incarnation. The operation claim becomes agent-owned in
/// the same transaction as the terminal result.
pub fn mark_registered(
    operation_id: &str,
    attempt_id: &str,
    provider_session_id: Option<&str>,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let Some((agent_id, name, provider, class, expected_provider_session_id, state)) = tx
            .query_row(
                "SELECT planned_agent_id, session_name, provider, class, COALESCE(provider_session_id,
                 CASE WHEN first_native_published = 1 THEN first_native_session_id END), state FROM agent_spawn_operations
                 WHERE operation_id = ?1 AND attempt_id = ?2",
                params![operation_id, attempt_id],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                )),
            )
            .optional()?
        else {
            tx.commit()?;
            return Ok(false);
        };
        if state != "launching"
            || expected_provider_session_id.as_deref() != provider_session_id
        {
            tx.commit()?;
            return Ok(false);
        }
        let persisted = tx
            .query_row(
                "SELECT session_name, provider, agent_class FROM agents WHERE session_id = ?1",
                params![agent_id],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                )),
            )
            .optional()?;
        if persisted.as_ref().is_none_or(|(persisted_name, persisted_provider, persisted_class)| {
            persisted_name != &name
                || persisted_provider.as_deref() != Some(provider.as_str())
                || persisted_class.as_deref() != Some(class.as_str())
        }) {
            tx.commit()?;
            return Ok(false);
        }
        let owned_claim: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_name_claims
             WHERE session_name = ?1 AND owner_kind = 'operation' AND owner_id = ?2)",
            params![name, operation_id],
            |row| row.get(0),
        )?;
        if !owned_claim {
            tx.commit()?;
            return Ok(false);
        }
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = tx.execute(
            "UPDATE agent_spawn_operations SET state = 'registered', updated_at = ?3,
                 error_code = NULL, error_message = NULL
             WHERE operation_id = ?1 AND attempt_id = ?2 AND state = 'launching'",
            params![operation_id, attempt_id, now],
        )?;
        if changed == 1 {
            let promoted = tx.execute(
                "UPDATE agent_name_claims SET owner_kind = 'agent', owner_id = ?2
                 WHERE session_name = ?1 AND owner_kind = 'operation' AND owner_id = ?3",
                params![name, agent_id, operation_id],
            )?;
            if promoted != 1 {
                return Err(std::io::Error::other(
                    "spawn operation name claim changed during registration",
                )
                .into());
            }
        }
        tx.commit()?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

pub fn restore_is_excluded(agent_id: &str) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| Ok(restore_is_excluded_with_conn(conn, agent_id)?))
        .map_err(|error| error.to_string())
}

/// Record first discovery only after the watcher proves its current config Arc
/// and runtime generation under lifecycle and roster exclusion. The original
/// registration native ID stays immutable; later replacements never overwrite
/// this separate receipt. Agents without a durable operation remain unaffected.
pub fn record_first_native_binding(
    agent_id: &str,
    provider: &str,
    native_id: &str,
    generation: u64,
) -> Result<bool, String> {
    if !matches!(provider, "opencode" | "antigravity") {
        return Err("Provider does not use first-session discovery".into());
    }
    crate::db::get_db_conn(|conn| {
        conn.execute("UPDATE agent_spawn_operations SET first_native_session_id = ?3, first_native_generation = ?4
            WHERE planned_agent_id = ?1 AND provider = ?2 AND provider_session_id IS NULL
            AND first_native_session_id IS NULL AND first_native_allowed_generation = ?4
            AND state IN ('launching', 'registered')",
            params![agent_id, provider, native_id, generation])?;
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_spawn_operations
            WHERE planned_agent_id = ?1 AND first_native_session_id = ?2 AND first_native_generation = ?3)",
            params![agent_id, native_id, generation], |row| row.get(0))?)
    }).map_err(|error| error.to_string())
}

/// Admit first discovery for the launched runtime or an exact saved-config
/// restore. A fresh-session replacement cannot inherit an unassigned operation.
pub fn record_first_native_runtime(
    config: &crate::models::AgentConfig,
    generation: u64,
    restored: bool,
) -> Result<(), String> {
    if !matches!(config.provider.as_str(), "opencode" | "antigravity")
        || config
            .resume_session
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty())
        || config.fresh_provider_session_id.is_some()
    {
        return Ok(());
    }
    crate::db::get_db_conn(|conn| {
        conn.execute(
            "UPDATE agent_spawn_operations SET first_native_allowed_generation = ?5
            WHERE planned_agent_id = ?1 AND provider = ?2 AND session_name = ?3
            AND class = ?4 AND prepared_workspace = ?6 AND provider_session_id IS NULL
            AND first_native_session_id IS NULL
            AND ((state = 'launching' AND first_native_allowed_generation IS NULL)
                OR (state = 'registered' AND ?7))",
            params![
                config.session_id,
                config.provider,
                config.session_name,
                config.agent_class,
                generation,
                config.folder,
                restored
            ],
        )?;
        Ok(())
    })
    .map_err(|error| error.to_string())
}

/// Validate a first discovery against the exact registration incarnation.
pub fn first_native_binding_matches(
    agent_id: &str,
    native_id: &str,
    generation: u64,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_spawn_operations WHERE planned_agent_id = ?1
         AND provider_session_id IS NULL AND first_native_session_id = ?2 AND first_native_generation = ?3
         AND first_native_published = 1)",
        params![agent_id, native_id, generation], |row| row.get(0))?)).map_err(|error| error.to_string())
}

/// Complete metadata only after the exact proposed JSON snapshot was written.
pub fn mark_first_native_published(
    agent_id: &str,
    native_id: &str,
    generation: u64,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| Ok(conn.execute(
        "UPDATE agent_spawn_operations SET first_native_published = 1
         WHERE planned_agent_id = ?1 AND first_native_session_id = ?2 AND first_native_generation = ?3",
        params![agent_id, native_id, generation])? == 1)).map_err(|error| error.to_string())
}

/// An interrupted strict publication can finish from actual saved identity
/// evidence. Missing or incompatible JSON remains quarantined, without replay.
pub fn recover_first_native_bindings(roster: &[crate::models::AgentConfig]) -> Result<(), String> {
    crate::db::get_db_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        for config in roster {
            if roster.iter().filter(|saved| saved.session_id == config.session_id
                || saved.session_name == config.session_name).count() != 1 { continue; }
            let native = config.resume_session.as_deref().filter(|id| !id.trim().is_empty())
                .or(config.fresh_provider_session_id.as_deref());
            tx.execute("UPDATE agent_spawn_operations SET first_native_published = 1
                WHERE planned_agent_id = ?1 AND session_name = ?2 AND provider = ?3 AND class = ?4
                AND prepared_workspace = ?5 AND first_native_session_id = ?6 AND first_native_published = 0",
                params![config.session_id, config.session_name, config.provider, config.agent_class, config.folder, native])?;
        }
        tx.commit()?;
        Ok(())
    }).map_err(|error| error.to_string())
}

fn restore_is_excluded_with_conn(conn: &Connection, agent_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_spawn_operations
         WHERE planned_agent_id = ?1 AND state IN
         ('queued', 'preparing', 'launching', 'outcome_unknown'))
         OR EXISTS(SELECT 1 FROM agent_spawn_operations WHERE planned_agent_id = ?1
             AND first_native_session_id IS NOT NULL AND first_native_published = 0)
         OR EXISTS(SELECT 1 FROM legacy_name_attempts WHERE agent_id = ?1
         AND (phase IN ('reserved', 'effects_possible', 'completed')
             OR removal_state IN ('intent', 'closure_proved')))",
        params![agent_id],
        |row| row.get(0),
    )
}

pub fn claimed_names() -> Result<HashSet<String>, String> {
    crate::db::get_db_conn(|conn| {
        let mut names = HashSet::new();
        let mut statement = conn.prepare("SELECT session_name FROM agent_name_claims")?;
        names.extend(
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        Ok(names)
    })
    .map_err(|error| error.to_string())
}

pub fn release_legacy_name(name: &str, owner_id: &str) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| legacy_names::release_unstarted_with_conn(conn, name, owner_id))
        .map_err(|error| error.to_string())
}

fn update_attempt_state(
    operation_id: &str,
    attempt_id: &str,
    from: &str,
    to: &str,
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = conn.execute(
            "UPDATE agent_spawn_operations SET state = ?4, updated_at = ?5
             WHERE operation_id = ?1 AND attempt_id = ?2 AND state = ?3",
            params![operation_id, attempt_id, from, to, now],
        )?;
        Ok(changed == 1)
    })
    .map_err(|error| error.to_string())
}

fn load_plan_with_conn(
    conn: &Connection,
    operation_id: &str,
) -> rusqlite::Result<Option<AgentSpawnPlan>> {
    let Some(operation) = load_operation_with_conn(conn, operation_id)? else {
        return Ok(None);
    };
    let (request_json, provider_session_id, attempt_id, prepared_workspace, first_native_session_id, first_native_published): (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
    ) = conn.query_row(
        "SELECT request_json, provider_session_id, attempt_id, prepared_workspace, first_native_session_id, first_native_published
             FROM agent_spawn_operations WHERE operation_id = ?1",
        params![operation_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    )?;
    let request = serde_json::from_str(&request_json)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    Ok(Some(AgentSpawnPlan {
        operation,
        request,
        provider_session_id,
        attempt_id,
        prepared_workspace,
        first_native_session_id,
        first_native_published,
    }))
}

fn load_operation_with_conn(
    conn: &Connection,
    operation_id: &str,
) -> rusqlite::Result<Option<AgentSpawnOperation>> {
    conn.query_row(
        "SELECT operation_id, planned_agent_id, session_name, provider, class,
                state, created_at, updated_at, error_code, error_message
         FROM agent_spawn_operations WHERE operation_id = ?1",
        params![operation_id],
        |row| {
            Ok(AgentSpawnOperation {
                operation_id: row.get(0)?,
                agent_id: row.get(1)?,
                agent_name: row.get(2)?,
                provider: row.get(3)?,
                class: row.get(4)?,
                state: AgentSpawnState::parse(&row.get::<_, String>(5)?)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
                error_code: row.get(8)?,
                error_message: row.get(9)?,
            })
        },
    )
    .optional()
}

fn canonical_uuid(value: &str, label: &str) -> Result<String, String> {
    uuid::Uuid::parse_str(value)
        .map(|id| id.hyphenated().to_string())
        .map_err(|_| format!("invalid {label}"))
}

pub fn canonical_operation_id(value: &str) -> Result<String, String> {
    canonical_uuid(value, "operation id")
}

pub fn new_operation_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

pub fn validate_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Agent name cannot be empty when specified.".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "Agent name must be {MAX_NAME_CHARS} characters or fewer."
        ));
    }
    if name.chars().any(char::is_whitespace) {
        return Err("Agent name may contain only letters, numbers, underscores, or hyphens; spaces are not allowed.".into());
    }
    let mut chars = name.chars();
    let first = chars
        .next()
        .ok_or_else(|| "Agent name cannot be empty when specified.".to_string())?;
    if !first.is_ascii_alphanumeric() {
        return Err("Agent name must start with a letter or number; leading hyphens are reserved by the CLI.".into());
    }
    if chars.any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')) {
        return Err("Agent name may contain only letters, numbers, underscores, or hyphens; spaces are not allowed.".into());
    }
    if name.eq_ignore_ascii_case("all") {
        return Err("Agent name 'all' is reserved for broadcast commands.".into());
    }
    if uuid::Uuid::parse_str(name).is_ok() {
        return Err("UUID-shaped agent names are reserved for session identifiers.".into());
    }
    Ok(())
}

fn resolve_requested_spawn_name_with_conn(
    conn: &Connection,
    requested: &str,
    class: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    if requested.trim().is_empty() {
        return Ok(generated_agent_name_with_conn(conn, class)?);
    }
    validate_agent_name(requested)
        .map_err(|error| Box::new(std::io::Error::other(error)) as Box<dyn std::error::Error>)?;
    if name_is_claimed(conn, requested)? {
        return Err(std::io::Error::other(format!(
            "An agent with the name '{requested}' already exists."
        ))
        .into());
    }
    Ok(requested.to_string())
}

fn generated_agent_name_with_conn(conn: &Connection, class: &str) -> rusqlite::Result<String> {
    let base = generated_agent_name_base(class);
    for suffix in NAME_SUFFIXES {
        let candidate = format!("{base}-{suffix}");
        if !name_is_claimed(conn, &candidate)? {
            return Ok(candidate);
        }
    }
    for cycle in 2..=MAX_GENERATED_NAME_CYCLES {
        for suffix in NAME_SUFFIXES {
            let candidate = format!("{base}-{suffix}-{cycle}");
            if !name_is_claimed(conn, &candidate)? {
                return Ok(candidate);
            }
        }
    }
    loop {
        let candidate = format!("{base}-agent-{}", uuid::Uuid::new_v4().simple());
        if !name_is_claimed(conn, &candidate)? {
            return Ok(candidate);
        }
    }
}

fn generated_agent_name_base(class: &str) -> String {
    let mut base = String::new();
    let mut separator = false;
    for ch in class.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            base.push(ch);
            separator = false;
        } else if !separator && !base.is_empty() {
            base.push('-');
            separator = true;
        }
    }
    let base = base.trim_matches('-');
    let mut base: String = if base.is_empty() {
        "Agent".into()
    } else {
        base.into()
    };
    let max_base = MAX_NAME_CHARS - "-agent-".len() - 32;
    base = base.chars().take(max_base).collect();
    base
}

fn name_is_claimed(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM agents WHERE session_name = ?1
            UNION ALL
            SELECT 1 FROM agent_name_claims WHERE session_name = ?1
         )",
        params![name],
        |row| row.get(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    pub(super) fn initialize_test_db(conn: &Connection) {
        conn.execute(
            "CREATE TABLE agents(session_id TEXT PRIMARY KEY, session_name TEXT UNIQUE)",
            [],
        )
        .unwrap();
        migrate(conn).unwrap();
    }

    pub(super) fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        initialize_test_db(&conn);
        conn
    }

    fn admit_fixture(
        conn: &Connection,
        operation_id: &str,
        request: &AgentSpawnRequest,
        planned_agent_id: &str,
    ) -> Result<Admission, Box<dyn std::error::Error>> {
        let request_json = serde_json::to_string(request)?;
        let payload_hash = format!("{:x}", Sha256::digest(request_json.as_bytes()));
        admit_with_conn(
            conn,
            operation_id,
            request,
            planned_agent_id,
            Some("provider-session"),
            &request_json,
            &payload_hash,
        )
    }

    fn request(name: Option<&str>) -> AgentSpawnRequest {
        AgentSpawnRequest {
            provider: "codex".into(),
            class: "Reviewer".into(),
            name: name.map(str::to_string),
            workspace: Some("<workspace>".into()),
            model: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn same_key_is_idempotent_and_payload_change_conflicts() {
        let conn = test_db();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let planned_id = uuid::Uuid::new_v4().to_string();
        let original = request(Some("reviewer-a"));
        let first = admit_fixture(&conn, &operation_id, &original, &planned_id).unwrap();
        let retry = admit_fixture(
            &conn,
            &operation_id,
            &original,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert!(first.created);
        assert!(!retry.created);
        assert_eq!(retry.operation, first.operation);
        assert_eq!(retry.operation.agent_id, planned_id);

        let changed = request(Some("reviewer-b"));
        let error = admit_fixture(
            &conn,
            &operation_id,
            &changed,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("different spawn request"));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_spawn_operations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn concurrent_exact_retries_share_one_plan() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let setup = Connection::open(&path).unwrap();
        setup
            .busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        initialize_test_db(&setup);
        drop(setup);

        let operation_id = uuid::Uuid::new_v4().to_string();
        let planned_id = uuid::Uuid::new_v4().to_string();
        let payload = request(Some("reviewer-concurrent"));
        let barrier = Arc::new(Barrier::new(2));
        let workers = (0..2)
            .map(|_| {
                let path = path.clone();
                let operation_id = operation_id.clone();
                let planned_id = planned_id.clone();
                let payload = payload.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(path).unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    barrier.wait();
                    admit_fixture(&conn, &operation_id, &payload, &planned_id).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.created).count(), 1);
        assert_eq!(results[0].operation, results[1].operation);
        assert_eq!(results[0].operation.agent_id, planned_id);
    }

    #[test]
    fn deduplication_precedes_queue_saturation_and_new_work_is_rejected() {
        let conn = test_db();
        let first_request = request(None);
        let first_operation_id = uuid::Uuid::new_v4().to_string();
        let first_plan_id = uuid::Uuid::new_v4().to_string();
        let first =
            admit_fixture(&conn, &first_operation_id, &first_request, &first_plan_id).unwrap();
        for _ in 1..MAX_QUEUED_OPERATIONS {
            admit_fixture(
                &conn,
                &uuid::Uuid::new_v4().to_string(),
                &request(None),
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
        }
        let retry = admit_fixture(
            &conn,
            &first_operation_id,
            &first_request,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert!(!retry.created);
        assert_eq!(retry.operation, first.operation);

        let error = admit_fixture(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            &request(None),
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("queue is full"));
    }

    #[test]
    fn restore_exclusion_tracks_only_unresolved_or_unknown_operations() {
        let conn = test_db();
        let request = request(None);
        let admission = admit_fixture(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            &request,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        let agent_id = admission.operation.agent_id;
        assert!(restore_is_excluded_with_conn(&conn, &agent_id).unwrap());
        conn.execute(
            "UPDATE agent_spawn_operations SET state = 'failed' WHERE planned_agent_id = ?1",
            params![agent_id],
        )
        .unwrap();
        assert!(!restore_is_excluded_with_conn(&conn, &agent_id).unwrap());
        conn.execute(
            "UPDATE agent_spawn_operations SET state = 'outcome_unknown' WHERE planned_agent_id = ?1",
            params![agent_id],
        )
        .unwrap();
        assert!(restore_is_excluded_with_conn(&conn, &agent_id).unwrap());
    }

    #[test]
    fn migration_creates_claims_and_operation_indexes_idempotently() {
        let conn = test_db();
        migrate(&conn).unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
             AND name IN ('agent_spawn_operations', 'agent_name_claims', 'legacy_name_attempts')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3);
        let columns = conn
            .prepare("PRAGMA table_info(agent_spawn_operations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<HashSet<_>>>()
            .unwrap();
        assert!(columns.contains("launch_context_json"));
        assert!(columns.contains("bootstrap_receipt_json"));
        let indexes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index'
                 AND name IN ('idx_agent_spawn_queue', 'idx_agent_spawn_planned_agent',
                              'idx_agent_name_claim_owner', 'idx_legacy_name_agent')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexes, 4);
    }

    #[test]
    fn interrupted_attempts_keep_names_and_only_never_claimed_work_is_queued() {
        let _guard = crate::tests::env_lock();
        let home = tempfile::tempdir().unwrap();
        crate::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let mut ids = Vec::new();
        for name in ["Queued", "Preparing", "Launching", "Rejected"] {
            let id = uuid::Uuid::new_v4().to_string();
            admit(
                &id,
                &request(Some(name)),
                &uuid::Uuid::new_v4().to_string(),
                Some("provider-session"),
            )
            .unwrap();
            ids.push(id);
        }
        let preparing = claim_queued(&ids[1]).unwrap().unwrap();
        let preparing_attempt = preparing.attempt_id.unwrap();
        assert!(claim_queued(&ids[1]).unwrap().is_none());
        assert!(!mark_launching(&ids[1], &uuid::Uuid::new_v4().to_string()).unwrap());
        let launching = claim_queued(&ids[2]).unwrap().unwrap();
        let launching_attempt = launching.attempt_id.unwrap();
        assert!(mark_launching(&ids[2], &launching_attempt).unwrap());
        let rejected = claim_queued(&ids[3]).unwrap().unwrap();
        assert!(fail_before_launch(
            &ids[3],
            rejected.attempt_id.as_deref().unwrap(),
            "invalid_request",
            "fixture rejection"
        )
        .unwrap());
        assert!(!claimed_names().unwrap().contains("Rejected"));
        assert_eq!(recover_after_owner_acquired().unwrap(), [ids[0].clone()]);
        assert_eq!(
            get(&ids[1]).unwrap().unwrap().state,
            AgentSpawnState::OutcomeUnknown
        );
        assert_eq!(
            get(&ids[2]).unwrap().unwrap().state,
            AgentSpawnState::OutcomeUnknown
        );
        assert!(claim_queued(&ids[1]).unwrap().is_none());
        assert!(claim_queued(&ids[2]).unwrap().is_none());
        assert!(!fail_before_launch(&ids[1], &preparing_attempt, "late", "late failure").unwrap());
        assert!(!mark_registered(&ids[2], &launching_attempt, Some("provider-session")).unwrap());
        assert!(claimed_names().unwrap().contains("Preparing"));
        assert!(claimed_names().unwrap().contains("Launching"));
        assert!(crate::db::get_all_agents().unwrap().is_empty());
    }

    #[test]
    fn operation_registration_and_name_promotion_rollback_together() {
        let _guard = crate::tests::env_lock();
        let home = tempfile::tempdir().unwrap();
        crate::db::init_db_at_path(&home.path().join("state.db")).unwrap();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let agent_id = uuid::Uuid::new_v4().to_string();
        let request = request(Some("Registered"));
        admit(&operation_id, &request, &agent_id, Some("provider-session")).unwrap();
        let plan = claim_queued(&operation_id).unwrap().unwrap();
        let attempt = plan.attempt_id.unwrap();
        assert!(mark_launching(&operation_id, &attempt).unwrap());
        assert!(!mark_registered(&operation_id, &attempt, Some("provider-session")).unwrap());
        crate::db::upsert_agent(&crate::db::AgentUpsert {
            session_id: &agent_id,
            session_name: "Registered",
            description: "",
            agent_class: "Reviewer",
            provider: "codex",
            workspace: None,
            project: None,
            is_off: false,
            created_at: None,
        })
        .unwrap();
        crate::db::get_db_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER cut_operation_promotion BEFORE UPDATE ON agent_name_claims
                BEGIN SELECT RAISE(ABORT, 'promotion unavailable'); END;",
            )?;
            Ok(())
        })
        .unwrap();
        assert!(mark_registered(&operation_id, &attempt, Some("provider-session")).is_err());
        assert_eq!(
            get(&operation_id).unwrap().unwrap().state,
            AgentSpawnState::Launching
        );
        assert!(restore_is_excluded(&agent_id).unwrap());
        crate::db::get_db_conn(|conn| {
            let owner: String = conn.query_row(
                "SELECT owner_kind FROM agent_name_claims WHERE session_name = 'Registered'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(owner, "operation");
            conn.execute_batch("DROP TRIGGER cut_operation_promotion")?;
            Ok(())
        })
        .unwrap();
        assert!(!mark_registered(
            &operation_id,
            &uuid::Uuid::new_v4().to_string(),
            Some("provider-session")
        )
        .unwrap());
        assert!(mark_registered(&operation_id, &attempt, Some("provider-session")).unwrap());
        assert_eq!(
            get(&operation_id).unwrap().unwrap().state,
            AgentSpawnState::Registered
        );
        assert!(!restore_is_excluded(&agent_id).unwrap());
        let retry = admit(
            &operation_id,
            &request,
            &uuid::Uuid::new_v4().to_string(),
            Some("different-new-plan"),
        )
        .unwrap();
        assert!(!retry.created);
        assert_eq!(retry.operation.agent_id, agent_id);
        assert_eq!(retry.operation.state, AgentSpawnState::Registered);
    }

    #[test]
    fn generated_name_uses_existing_phonetic_suffix_and_validation_rules() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO agents VALUES ('existing', 'Reviewer-alpha')",
            [],
        )
        .unwrap();
        assert_eq!(
            generated_agent_name_with_conn(&conn, "Reviewer").unwrap(),
            "Reviewer-bravo"
        );
        assert!(validate_agent_name("-reviewer").is_err());
    }
}
