//! Explicit, metadata-only recovery of an exactly registered Codex spawn.

use super::{canonical_uuid, load_plan_with_conn, AgentSpawnPlan, AgentSpawnState};
use crate::models::AgentConfig;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    FinalizeRegistered,
    LaunchIfAbsent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub operation_id: String,
    pub expected_attempt_id: String,
    pub expected_model: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finalized {
    pub schema: u8,
    pub operation_id: String,
    pub original_attempt_id: String,
    pub agent_id: String,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blocker {
    pub code: &'static str,
    pub message: &'static str,
}

impl Blocker {
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

impl From<rusqlite::Error> for Blocker {
    fn from(_: rusqlite::Error) -> Self {
        storage_unavailable()
    }
}

fn storage_unavailable() -> Blocker {
    Blocker::new(
        "spawn_recovery_storage_unavailable",
        "Durable spawn evidence could not be read or committed.",
    )
}

pub(super) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_spawn_recovery_audit (
            operation_id TEXT PRIMARY KEY,
            original_attempt_id TEXT NOT NULL,
            original_error_code TEXT,
            original_error_message TEXT,
            confirmed_model TEXT NOT NULL,
            native_session_id TEXT NOT NULL,
            provider_generation INTEGER NOT NULL,
            terminal_generation INTEGER NOT NULL,
            finalized_at TEXT NOT NULL
        )",
    )
}

/// Only the application owner can supply current broker and roster evidence.
/// This value is internal to the service and never deserialized from RPC.
pub struct RegistrationEvidence {
    pub config: AgentConfig,
    pub terminal_generation: u64,
    pub provider_generation: u64,
}

pub fn validate_request(request: &Request) -> Result<(), Blocker> {
    for (id, label) in [
        (&request.operation_id, "operation id"),
        (&request.expected_attempt_id, "attempt id"),
    ] {
        if canonical_uuid(id, label).as_ref() != Ok(id) {
            return Err(Blocker::new(
                "spawn_recovery_invalid_identity",
                "Expected canonical operation and attempt UUIDs.",
            ));
        }
    }
    let model = &request.expected_model;
    if model.is_empty()
        || model.chars().count() > 256
        || model.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Blocker::new(
            "spawn_recovery_invalid_model",
            "Supply one exact, nonempty model identifier.",
        ));
    }
    Ok(())
}

/// Resolve the exact uncertain operation and its unchanged durable claim.
/// Legacy attempts cannot establish that provider effects never began.
pub fn plan_for_recovery(request: &Request) -> Result<AgentSpawnPlan, Blocker> {
    validate_request(request)?;
    crate::db::get_db_conn(|conn| Ok(read_plan(conn, request)))
        .map_err(|_| storage_unavailable())?
}

fn read_plan(conn: &Connection, request: &Request) -> Result<AgentSpawnPlan, Blocker> {
    let plan = load_plan_with_conn(conn, &request.operation_id)?.ok_or_else(|| {
        Blocker::new(
            "spawn_recovery_operation_missing",
            "The specified spawn operation does not exist.",
        )
    })?;
    if plan.attempt_id.as_deref() != Some(&request.expected_attempt_id) {
        return Err(Blocker::new(
            "spawn_recovery_attempt_changed",
            "The durable attempt no longer matches the expected attempt.",
        ));
    }
    if plan.operation.state != AgentSpawnState::OutcomeUnknown {
        return Err(Blocker::new(
            "spawn_recovery_not_uncertain",
            "The specified attempt is no longer outcome_unknown.",
        ));
    }
    if plan.operation.provider != "codex" || plan.request.provider != "codex" {
        return Err(Blocker::new(
            "spawn_recovery_provider_unsupported",
            "This recovery route requires a Codex operation.",
        ));
    }
    let (original_json, hash): (String, String) = conn.query_row(
        "SELECT request_json, payload_hash FROM agent_spawn_operations WHERE operation_id=?1",
        [&request.operation_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if format!("{:x}", Sha256::digest(original_json.as_bytes())) != hash {
        return Err(Blocker::new(
            "spawn_recovery_request_unverified",
            "The original request and its durable hash do not agree.",
        ));
    }
    let owned: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_name_claims
         WHERE session_name=?1 AND owner_kind='operation' AND owner_id=?2)",
        params![plan.operation.agent_name, request.operation_id],
        |row| row.get(0),
    )?;
    if !owned {
        return Err(Blocker::new(
            "spawn_recovery_claim_changed",
            "The operation no longer owns the exact agent name claim.",
        ));
    }
    let summary = crate::db::get_agent_by_session_id_with_conn(conn, &plan.operation.agent_id)?;
    if plan.prepared_workspace.as_deref().is_none_or(str::is_empty)
        || !summary.is_some_and(|row| {
            row.session_name == plan.operation.agent_name
                && row.provider.as_deref() == Some("codex")
                && row.agent_class.as_deref() == Some(&plan.operation.class)
                && row.workspace == plan.prepared_workspace
                && !row.is_off
        })
    {
        return Err(Blocker::new(
            "spawn_recovery_summary_unverified",
            "The prepared workspace and enabled SQLite agent identity do not agree.",
        ));
    }
    if request.action == Action::LaunchIfAbsent {
        return Err(Blocker::new("spawn_recovery_launch_absence_unverified", "The original attempt has no authoritative pre-effect proof. An unrecorded Codex provider effect cannot be excluded."));
    }
    if plan.request.model.as_deref() != Some(&request.expected_model) {
        return Err(Blocker::new(
            "spawn_recovery_model_mismatch",
            "The expected model differs from the original spawn model.",
        ));
    }
    if plan
        .provider_session_id
        .as_deref()
        .is_none_or(|id| canonical_uuid(id, "Codex native id").as_deref() != Ok(id))
    {
        return Err(Blocker::new(
            "spawn_recovery_native_unverified",
            "The operation has no exact recorded Codex native session.",
        ));
    }
    Ok(plan)
}

/// Commit only after the caller pins the same live registration under the
/// lifecycle, roster and native-owner guards. The persistent lease check and
/// SQLite compare-and-set are one admission boundary.
pub fn finalize_registered(
    request: &Request,
    plan: &AgentSpawnPlan,
    evidence: &RegistrationEvidence,
    native_still_idle: impl FnOnce() -> bool,
) -> Result<Finalized, Blocker> {
    validate_request(request)?;
    let session = evidence.config.resume_session.as_deref().ok_or_else(|| {
        Blocker::new(
            "spawn_recovery_native_unverified",
            "The current registration has no native Codex session.",
        )
    })?;
    crate::conversation_lease::inspect_leases_locked(|leases| {
        if crate::conversation_lease::find_active_conflict(
            leases,
            &plan.operation.agent_id,
            session,
            &chrono::Utc::now().to_rfc3339(),
        )
        .is_some()
        {
            return Err(Blocker::new(
                "spawn_recovery_lease_conflict",
                "A persisted conversation owner conflicts with this recovery.",
            ));
        }
        crate::db::get_db_conn(|conn| {
            Ok(finalize_with_conn(
                conn,
                request,
                plan,
                evidence,
                session,
                native_still_idle,
            ))
        })
        .map_err(|_| storage_unavailable())?
    })
    .map_err(|_| {
        Blocker::new(
            "spawn_recovery_leases_unavailable",
            "Persisted conversation ownership could not be verified.",
        )
    })?
}

fn finalize_with_conn(
    conn: &Connection,
    request: &Request,
    expected: &AgentSpawnPlan,
    evidence: &RegistrationEvidence,
    session: &str,
    native_still_idle: impl FnOnce() -> bool,
) -> Result<Finalized, Blocker> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let current = read_plan(&tx, request)?;
    if current != *expected {
        return Err(Blocker::new(
            "spawn_recovery_intent_changed",
            "The durable operation changed while registration evidence was collected.",
        ));
    }
    let operation = &current.operation;
    let config = &evidence.config;
    if config.session_id != operation.agent_id
        || config.session_name != operation.agent_name
        || config.provider != "codex"
        || config.agent_class != operation.class
        || Some(config.folder.as_str()) != current.prepared_workspace.as_deref()
        || config.is_off
        || config.fresh_provider_session_id.is_some()
    {
        return Err(Blocker::new(
            "spawn_recovery_registration_mismatch",
            "The live registration differs from the prepared identity.",
        ));
    }
    if config.model.as_deref() != Some(&request.expected_model) {
        return Err(Blocker::new(
            "spawn_recovery_model_mismatch",
            "The registered agent has a different model.",
        ));
    }
    let generation = i64::try_from(evidence.provider_generation)
        .ok()
        .filter(|n| *n > 0);
    let runtime = i64::try_from(evidence.terminal_generation)
        .ok()
        .filter(|n| *n > 0);
    let binding = crate::db::latest_native_session_binding_with_conn(&tx, &operation.agent_id)?;
    if current.provider_session_id.as_deref() != Some(session)
        || generation.is_none()
        || runtime.is_none()
        || !binding.is_some_and(|binding| {
            binding.target_agent_id == operation.agent_id
                && binding.generation == evidence.provider_generation
                && binding.provider == "codex"
                && binding.transport == "codex_app_server_ws"
                && binding.provider_session_id.as_deref() == Some(session)
        })
        || !native_still_idle()
    {
        return Err(Blocker::new(
            "spawn_recovery_native_unverified",
            "The live Codex owner and durable native session binding do not agree.",
        ));
    }
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let changed = tx.execute(
        "UPDATE agent_spawn_operations SET state='registered', updated_at=?3, error_code=NULL, error_message=NULL
         WHERE operation_id=?1 AND attempt_id=?2 AND state='outcome_unknown'",
        params![request.operation_id, request.expected_attempt_id, now],
    )?;
    let promoted = tx.execute(
        "UPDATE agent_name_claims SET owner_kind='agent', owner_id=?2
         WHERE session_name=?1 AND owner_kind='operation' AND owner_id=?3",
        params![
            operation.agent_name,
            operation.agent_id,
            request.operation_id
        ],
    )?;
    if changed != 1 || promoted != 1 {
        return Err(Blocker::new(
            "spawn_recovery_claim_changed",
            "The operation or exact name claim changed before finalization.",
        ));
    }
    tx.execute(
        "INSERT INTO agent_spawn_recovery_audit VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            request.operation_id,
            request.expected_attempt_id,
            operation.error_code,
            operation.error_message,
            request.expected_model,
            session,
            generation,
            runtime,
            now
        ],
    )?;
    tx.commit()?;
    Ok(Finalized {
        schema: crate::control::CONTROL_SCHEMA,
        operation_id: request.operation_id.clone(),
        original_attempt_id: request.expected_attempt_id.clone(),
        agent_id: operation.agent_id.clone(),
        action: request.action,
    })
}
