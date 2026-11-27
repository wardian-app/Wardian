//! Explicit Codex spawn recovery and evidence from the pre-effect fence.

use super::{canonical_uuid, load_plan_with_conn, AgentSpawnPlan, AgentSpawnState};
use crate::models::AgentConfig;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
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
    pub confirmed_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchAccepted {
    pub schema: u8,
    pub operation_id: String,
    pub original_attempt_id: String,
    pub accepted_attempt_id: String,
    pub agent_id: String,
    pub action: Action,
    pub effective_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Finalized(Finalized),
    LaunchAccepted(LaunchAccepted),
}

pub enum Admission {
    Committed(Finalized),
    NeedsLiveProof(Box<AgentSpawnPlan>),
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
        );
        CREATE TABLE IF NOT EXISTS agent_spawn_codex_pre_effect_witnesses (
            operation_id TEXT PRIMARY KEY,
            original_attempt_id TEXT NOT NULL,
            planned_agent_id TEXT NOT NULL,
            payload_hash TEXT NOT NULL,
            session_name TEXT NOT NULL,
            class TEXT NOT NULL,
            prepared_workspace TEXT NOT NULL,
            launch_context_json TEXT NOT NULL,
            preparing_updated_at TEXT NOT NULL,
            witnessed_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_spawn_codex_launch_audit (
            operation_id TEXT PRIMARY KEY,
            original_attempt_id TEXT NOT NULL,
            accepted_attempt_id TEXT NOT NULL UNIQUE,
            effective_model TEXT NOT NULL,
            lease_acquisition_id TEXT NOT NULL,
            original_error_code TEXT,
            original_error_message TEXT,
            accepted_at TEXT NOT NULL
        )",
    )
}

/// Seal the exact preparation snapshot while the exclusive owner still sees
/// the old attempt before its launch fence. Quarantine uses the same transaction.
pub(super) fn record_pre_effect_witnesses(conn: &Connection, now: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO agent_spawn_codex_pre_effect_witnesses (
            operation_id, original_attempt_id, planned_agent_id, payload_hash, session_name,
            class, prepared_workspace, launch_context_json, preparing_updated_at, witnessed_at
         ) SELECT o.operation_id, o.attempt_id, o.planned_agent_id, o.payload_hash, o.session_name,
                  o.class, o.prepared_workspace, o.launch_context_json, o.updated_at, ?1
           FROM agent_spawn_operations o JOIN agent_name_claims c ON c.session_name = o.session_name
           WHERE o.state = 'preparing' AND o.provider = 'codex'
             AND o.codex_pre_effect_fence_version = 1 AND o.attempt_id IS NOT NULL
             AND length(o.prepared_workspace) > 0 AND length(o.launch_context_json) > 0
             AND o.provider_session_id IS NULL AND o.bootstrap_receipt_json IS NULL
             AND o.first_native_session_id IS NULL
             AND c.owner_kind = 'operation' AND c.owner_id = o.operation_id
           ON CONFLICT(operation_id) DO NOTHING",
        [now],
    )?;
    Ok(())
}

fn witnessed_launch(conn: &Connection, operation_id: &str) -> Result<String, Blocker> {
    conn.query_row(
        "SELECT w.launch_context_json FROM agent_spawn_codex_pre_effect_witnesses w
         JOIN agent_spawn_operations o ON o.operation_id = w.operation_id
         WHERE o.operation_id = ?1 AND o.attempt_id = w.original_attempt_id
           AND o.updated_at = w.witnessed_at AND o.error_code = 'interrupted_after_claim'
           AND o.codex_pre_effect_fence_version = 1 AND o.planned_agent_id = w.planned_agent_id
           AND o.payload_hash = w.payload_hash AND o.session_name = w.session_name
           AND o.class = w.class AND o.prepared_workspace = w.prepared_workspace
           AND o.launch_context_json = w.launch_context_json AND o.provider_session_id IS NULL
           AND o.bootstrap_receipt_json IS NULL AND o.first_native_session_id IS NULL",
        [operation_id],
        |row| row.get(0),
    ).optional()?.ok_or_else(|| {
        Blocker::new(
            "spawn_recovery_launch_absence_unverified",
            "The original attempt has no authoritative pre-effect proof. An unrecorded Codex provider effect cannot be excluded.",
        )
    })
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

/// Return an exact committed receipt before asking for any live evidence.
/// A read transaction keeps the receipt and operation lookup on one snapshot.
pub fn admit_recovery(request: &Request) -> Result<Admission, Blocker> {
    validate_request(request)?;
    crate::db::get_db_conn(|conn| Ok(admit_with_conn(conn, request)))
        .map_err(|_| storage_unavailable())?
}

fn admit_with_conn(conn: &Connection, request: &Request) -> Result<Admission, Blocker> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let admission = match read_receipt(&tx, request)? {
        Some(receipt) => Admission::Committed(receipt),
        None => Admission::NeedsLiveProof(Box::new(read_plan(&tx, request)?)),
    };
    tx.commit()?;
    Ok(admission)
}

/// Recheck after waiting for an admission lock another caller may have held
/// through its commit.
pub fn committed_receipt(request: &Request) -> Result<Option<Finalized>, Blocker> {
    validate_request(request)?;
    crate::db::get_db_conn(|conn| Ok(read_receipt(conn, request)))
        .map_err(|_| storage_unavailable())?
}

fn read_receipt(conn: &Connection, request: &Request) -> Result<Option<Finalized>, Blocker> {
    // This audit only records metadata finalization. The operation retains
    // its immutable planned agent ID even after the live agent is removed.
    let stored: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT a.original_attempt_id,a.confirmed_model,o.planned_agent_id
             FROM agent_spawn_recovery_audit a
             LEFT JOIN agent_spawn_operations o ON o.operation_id=a.operation_id
               AND o.attempt_id=a.original_attempt_id AND o.state='registered' AND o.provider='codex'
             WHERE a.operation_id=?1",
            [&request.operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((original_attempt_id, confirmed_model, agent_id)) = stored else {
        return Ok(None);
    };
    if request.action != Action::FinalizeRegistered
        || request.expected_attempt_id != original_attempt_id
        || request.expected_model != confirmed_model
    {
        return Err(Blocker::new(
            "spawn_recovery_receipt_mismatch",
            "The committed recovery belongs to a different action, attempt, or model.",
        ));
    }
    let agent_id = agent_id.ok_or_else(storage_unavailable)?;
    Ok(Some(Finalized {
        schema: crate::control::CONTROL_SCHEMA,
        operation_id: request.operation_id.clone(),
        original_attempt_id,
        agent_id,
        action: Action::FinalizeRegistered,
        confirmed_model,
    }))
}

/// Reproduce the committed launch acceptance without inspecting live state or
/// consuming another worker slot. Only the original request can replay it.
pub fn committed_launch_receipt(request: &Request) -> Result<Option<LaunchAccepted>, Blocker> {
    validate_request(request)?;
    crate::db::get_db_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
        Ok(read_launch_receipt(&tx, request))
    })
    .map_err(|_| storage_unavailable())?
}

fn read_launch_receipt(
    conn: &Connection,
    request: &Request,
) -> Result<Option<LaunchAccepted>, Blocker> {
    let stored: Option<(String, String, String, Option<String>)> = conn
        .query_row(
            "SELECT a.original_attempt_id, a.accepted_attempt_id, a.effective_model, o.planned_agent_id
             FROM agent_spawn_codex_launch_audit a LEFT JOIN agent_spawn_operations o
               ON o.operation_id = a.operation_id
             WHERE a.operation_id = ?1",
            [&request.operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((original_attempt_id, accepted_attempt_id, effective_model, agent_id)) = stored else {
        // A committed finalization also excludes new launch admission.
        let _ = read_receipt(conn, request)?;
        return Ok(None);
    };
    if request.action != Action::LaunchIfAbsent
        || request.expected_attempt_id != original_attempt_id
        || request.expected_model != effective_model
    {
        return Err(Blocker::new(
            "spawn_recovery_receipt_mismatch",
            "The committed recovery belongs to a different action, attempt, or model.",
        ));
    }
    Ok(Some(LaunchAccepted {
        schema: crate::control::CONTROL_SCHEMA,
        operation_id: request.operation_id.clone(),
        original_attempt_id,
        accepted_attempt_id,
        agent_id: agent_id.ok_or_else(storage_unavailable)?,
        action: Action::LaunchIfAbsent,
        effective_model,
    }))
}

fn read_uncertain(conn: &Connection, request: &Request) -> Result<AgentSpawnPlan, Blocker> {
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
    Ok(plan)
}

fn read_plan(conn: &Connection, request: &Request) -> Result<AgentSpawnPlan, Blocker> {
    if request.action != Action::FinalizeRegistered {
        return Err(Blocker::new(
            "spawn_recovery_invalid_action",
            "This route requires finalize-registered.",
        ));
    }
    let plan = read_uncertain(conn, request)?;
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

#[derive(Debug, PartialEq, Eq)]
pub struct WitnessedLaunch {
    pub plan: AgentSpawnPlan,
    pub launch_context_json: String,
}

fn read_launch(conn: &Connection, request: &Request) -> Result<WitnessedLaunch, Blocker> {
    if request.action != Action::LaunchIfAbsent {
        return Err(Blocker::new(
            "spawn_recovery_invalid_action",
            "This route requires launch-if-absent.",
        ));
    }
    if read_receipt(conn, request)?.is_some()
        || conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_spawn_codex_launch_audit WHERE operation_id=?1)",
            [&request.operation_id],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Err(Blocker::new(
            "spawn_recovery_witness_used",
            "The operation already has a committed recovery action.",
        ));
    }
    let plan = read_uncertain(conn, request)?;
    let launch_context_json = witnessed_launch(conn, &request.operation_id)?;
    if canonical_uuid(&plan.operation.agent_id, "agent id").as_ref() != Ok(&plan.operation.agent_id)
    {
        return Err(Blocker::new(
            "spawn_recovery_invalid_identity",
            "The planned agent identity is unavailable.",
        ));
    }
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE session_id=?1 OR session_name=?2 COLLATE NOCASE)
             OR EXISTS(SELECT 1 FROM native_session_bindings WHERE target_agent_id=?1)",
        params![plan.operation.agent_id, plan.operation.agent_name],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(Blocker::new(
            "spawn_recovery_agent_occupied",
            "The planned agent has existing durable roster or native ownership evidence.",
        ));
    }
    Ok(WitnessedLaunch {
        plan,
        launch_context_json,
    })
}

/// Read the pre-effect proof and durable vacancy from one storage snapshot.
pub fn plan_launch(request: &Request) -> Result<WitnessedLaunch, Blocker> {
    validate_request(request)?;
    crate::db::get_db_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
        Ok(read_launch(&tx, request))
    })
    .map_err(|_| storage_unavailable())?
}

/// Consume the witness and claim an unqueued attempt while the caller owns the
/// local lifecycle and roster barriers plus the exact empty-scope acquisition.
pub fn claim_recovery_launch(
    request: &Request,
    expected: &WitnessedLaunch,
    guard: &crate::conversation_lease::PersistedConversationLeaseGuard,
) -> Result<LaunchAccepted, Blocker> {
    validate_request(request)?;
    crate::conversation_lease::inspect_leases_locked(|leases| {
        let now = chrono::Utc::now();
        let agent = &expected.plan.operation.agent_id;
        let exact_lease = leases
            .iter()
            .filter(|lease| &lease.agent_id == agent)
            .all(|lease| {
                lease.owner() == *guard.owner()
                    || (lease.mode != "uncertain_previous_provider"
                        && chrono::DateTime::parse_from_rfc3339(&lease.expires_at)
                            .is_ok_and(|expiry| expiry <= now))
            })
            && leases.iter().any(|lease| {
                lease.owner() == *guard.owner()
                    && &lease.agent_id == agent
                    && lease.owner_kind == "agent_lifecycle"
                    && lease.provider == "codex"
                    && lease.mode == "lifecycle_transition"
                    && lease.resume_session.is_empty()
                    && chrono::DateTime::parse_from_rfc3339(&lease.expires_at)
                        .is_ok_and(|expiry| expiry > now)
            });
        if !exact_lease {
            return Err(Blocker::new(
                "spawn_recovery_lease_conflict",
                "The exact vacant-agent lease is unavailable or conflicts.",
            ));
        }
        crate::db::get_db_conn(|conn| {
            Ok(claim_launch_with_conn(
                conn,
                request,
                expected,
                &guard.owner().acquisition_id,
                &now.to_rfc3339(),
            ))
        })
        .map_err(|_| storage_unavailable())?
    })
    .map_err(|_| storage_unavailable())?
}

fn claim_launch_with_conn(
    conn: &Connection,
    request: &Request,
    expected: &WitnessedLaunch,
    lease_id: &str,
    now: &str,
) -> Result<LaunchAccepted, Blocker> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    if read_launch(&tx, request)? != *expected {
        return Err(Blocker::new(
            "spawn_recovery_intent_changed",
            "The pre-effect operation changed during admission.",
        ));
    }
    let attempt = super::new_operation_id();
    tx.execute(
        "INSERT INTO agent_spawn_codex_launch_audit VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            request.operation_id,
            request.expected_attempt_id,
            attempt,
            request.expected_model,
            lease_id,
            expected.plan.operation.error_code,
            expected.plan.operation.error_message,
            now
        ],
    )?;
    let changed = tx.execute(
        "UPDATE agent_spawn_operations SET state='preparing', attempt_id=?3, updated_at=?4,
            error_code=NULL,error_message=NULL,codex_pre_effect_fence_version=NULL
         WHERE operation_id=?1 AND attempt_id=?2 AND state='outcome_unknown'",
        params![
            request.operation_id,
            request.expected_attempt_id,
            attempt,
            now
        ],
    )?;
    if changed != 1 {
        return Err(storage_unavailable());
    }
    tx.commit()?;
    Ok(LaunchAccepted {
        schema: crate::control::CONTROL_SCHEMA,
        operation_id: request.operation_id.clone(),
        original_attempt_id: request.expected_attempt_id.clone(),
        agent_id: expected.plan.operation.agent_id.clone(),
        action: request.action,
        effective_model: request.expected_model.clone(),
        accepted_attempt_id: attempt,
    })
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
    if let Some(receipt) = committed_receipt(request)? {
        return Ok(receipt);
    }
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
    if let Some(receipt) = read_receipt(&tx, request)? {
        return Ok(receipt);
    }
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
    let receipt = read_receipt(&tx, request)?.ok_or_else(storage_unavailable)?;
    tx.commit()?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_recovery_writer_replays_when_a_receipt_already_won_admission() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::migrate(&conn).unwrap();
        let request = Request {
            operation_id: super::super::new_operation_id(),
            expected_attempt_id: super::super::new_operation_id(),
            expected_model: "fixture-model".into(),
            action: Action::FinalizeRegistered,
        };
        let agent_id = super::super::new_operation_id();
        let payload = serde_json::json!({
            "provider":"codex", "class":"Fixture", "name":"fixture",
            "workspace":"/fixture", "model":request.expected_model, "reasoning_effort":null,
        })
        .to_string();
        let hash = format!("{:x}", Sha256::digest(payload.as_bytes()));
        conn.execute(
            "INSERT INTO agent_spawn_operations (
                operation_id,payload_hash,request_json,planned_agent_id,session_name,provider,class,
                state,attempt_id,created_at,updated_at
             ) VALUES (?1,?2,?3,?4,'fixture','codex','Fixture','outcome_unknown',?5,'fixture','fixture')",
            params![request.operation_id, hash, payload, agent_id, request.expected_attempt_id],
        ).unwrap();
        let stale = load_plan_with_conn(&conn, &request.operation_id)
            .unwrap()
            .unwrap();
        let winner = conn.unchecked_transaction().unwrap();
        winner
            .execute(
                "UPDATE agent_spawn_operations SET state='registered' WHERE operation_id=?1",
                [&request.operation_id],
            )
            .unwrap();
        winner.execute(
            "INSERT INTO agent_spawn_recovery_audit VALUES (?1,?2,NULL,NULL,?3,'fixture-native',7,8,'fixture')",
            params![request.operation_id, request.expected_attempt_id, request.expected_model],
        ).unwrap();
        winner.commit().unwrap();

        // The contender had already chosen the plan and reached its writer
        // transaction. Nothing in its now-stale evidence is authoritative.
        let proof = RegistrationEvidence {
            config: AgentConfig::default(),
            terminal_generation: 0,
            provider_generation: 0,
        };
        let recorded = read_receipt(&conn, &request).unwrap().unwrap();
        let reply = finalize_with_conn(&conn, &request, &stale, &proof, "", || {
            panic!("the writer must replay before inspecting stale proof")
        })
        .unwrap();
        assert_eq!(reply, recorded);
        let count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_spawn_recovery_audit",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
