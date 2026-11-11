//! Exact, monotonic deletion dispositions for pending legacy name attempts.

use super::*;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

type StoreResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Non-secret identity of the actual separately saved roster incarnation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovalAgentIdentity {
    pub agent_id: String,
    pub name: String,
    pub provider: String,
    pub class: String,
    pub workspace: String,
    pub resume_session: Option<String>,
    pub fresh_provider_session_id: Option<String>,
}

impl From<&crate::models::AgentConfig> for RemovalAgentIdentity {
    fn from(config: &crate::models::AgentConfig) -> Self {
        Self {
            agent_id: config.session_id.clone(),
            name: config.session_name.clone(),
            provider: config.provider.clone(),
            class: config.agent_class.clone(),
            workspace: config.folder.clone(),
            resume_session: config.resume_session.clone(),
            fresh_provider_session_id: config.fresh_provider_session_id.clone(),
        }
    }
}

/// Actual filesystem object identity and digests of existing ownership records.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovalProfileIdentity {
    pub directory_id: Option<(u64, u64)>,
    pub ownership_records: BTreeMap<String, Option<String>>,
}

/// Sealed observations of the narrowly covered owned runtime, never PID proof.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovalClosureReceipt {
    pub runtime_generation: Option<u64>,
    pub owned_process_count: usize,
    pub all_process_handles_joined: bool,
    pub broker_closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DbIdentity {
    session_id: String,
    name: String,
    provider: Option<String>,
    class: Option<String>,
    workspace: Option<String>,
    project: Option<String>,
    created_at: Option<String>,
}

impl From<crate::db::AgentRow> for DbIdentity {
    fn from(row: crate::db::AgentRow) -> Self {
        Self {
            session_id: row.session_id,
            name: row.session_name,
            provider: row.provider,
            class: row.agent_class,
            workspace: row.workspace,
            project: row.project,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ClaimIdentity {
    name: String,
    kind: String,
    owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Binding {
    roster: RemovalAgentIdentity,
    database: DbIdentity,
    runtime_generation: Option<u64>,
    profile: RemovalProfileIdentity,
    claims: Vec<ClaimIdentity>,
    closure: Option<RemovalClosureReceipt>,
}

/// The existing attempt and absolute sequence are the only deletion token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoundRemovalTicket {
    pub agent_id: String,
    pub attempt_id: String,
    pub sequence: i64,
    pub state: String,
    binding: Binding,
}

impl BoundRemovalTicket {
    pub fn identity(&self) -> &RemovalAgentIdentity {
        &self.binding.roster
    }
    pub fn profile(&self) -> &RemovalProfileIdentity {
        &self.binding.profile
    }
    pub fn runtime_generation(&self) -> Option<u64> {
        self.binding.runtime_generation
    }
}

struct Attempt {
    sequence: i64,
    id: String,
    purpose: String,
    phase: String,
    name: String,
    state: Option<String>,
    binding: Option<String>,
}

fn latest(conn: &Connection, agent: &str) -> StoreResult<Option<Attempt>> {
    Ok(conn.query_row("SELECT sequence, attempt_id, purpose, phase, new_name, removal_state, removal_binding_json
        FROM legacy_name_attempts WHERE agent_id = ?1 ORDER BY sequence DESC LIMIT 1", params![agent], |row| {
        Ok(Attempt { sequence:row.get(0)?, id:row.get(1)?, purpose:row.get(2)?, phase:row.get(3)?,
            name:row.get(4)?, state:row.get(5)?, binding:row.get(6)? })
    }).optional()?)
}

fn claims(conn: &Connection, agent: &str) -> StoreResult<Vec<ClaimIdentity>> {
    let mut query = conn.prepare("SELECT session_name, owner_kind, owner_id FROM agent_name_claims
        WHERE (owner_kind = 'agent' AND owner_id = ?1) OR (owner_kind = 'legacy' AND owner_id IN
            (SELECT attempt_id FROM legacy_name_attempts WHERE agent_id = ?1)) ORDER BY session_name")?;
    let rows = query
        .query_map(params![agent], |row| {
            Ok(ClaimIdentity {
                name: row.get(0)?,
                kind: row.get(1)?,
                owner: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn ticket(agent: &str, attempt: Attempt) -> StoreResult<BoundRemovalTicket> {
    let state = attempt.state.ok_or("Deletion intent is missing")?;
    if !matches!(state.as_str(), "intent" | "closure_proved" | "done") {
        return Err("Deletion disposition is malformed".into());
    }
    Ok(BoundRemovalTicket {
        agent_id: agent.into(),
        attempt_id: attempt.id,
        sequence: attempt.sequence,
        state,
        binding: serde_json::from_str(&attempt.binding.ok_or("Deletion binding is missing")?)?,
    })
}

fn reject_older_pending(conn: &Connection, agent: &str, sequence: i64) -> StoreResult<()> {
    let pending: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM legacy_name_attempts
        WHERE agent_id = ?1 AND sequence != ?2 AND removal_state IN ('intent', 'closure_proved'))",
        params![agent, sequence],
        |row| row.get(0),
    )?;
    if pending {
        return Err("A stale deletion ticket remains pending".into());
    }
    Ok(())
}

/// Classify once before JSON publication. Storage failure never means ordinary.
/// The fence runs after SQL admission and must not acquire outer app locks.
pub fn admit_bound_removal(
    identity: &RemovalAgentIdentity,
    generation: Option<u64>,
    profile: &RemovalProfileIdentity,
    fence: impl FnOnce() -> Result<(), String>,
) -> Result<Option<BoundRemovalTicket>, String> {
    crate::db::get_db_conn(|conn| {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let Some(attempt) = latest(&tx, &identity.agent_id)? else {
            tx.commit()?;
            return Ok(None);
        };
        reject_older_pending(&tx, &identity.agent_id, attempt.sequence)?;
        if attempt.state.is_some() {
            let current = ticket(&identity.agent_id, attempt)?;
            if current.state != "intent"
                || &current.binding.roster != identity
                || current.binding.runtime_generation != generation
                || &current.binding.profile != profile
            {
                return Err("Deletion ticket has a different incarnation".into());
            }
            tx.commit()?;
            return Ok(Some(current));
        }
        if !matches!(
            attempt.phase.as_str(),
            "reserved" | "effects_possible" | "completed"
        ) {
            tx.commit()?;
            return Ok(None);
        }
        let database = crate::db::get_agent_by_session_id_with_conn(&tx, &identity.agent_id)?
            .ok_or("Bound deletion has no actual database identity")?
            .into();
        let claims = claims(&tx, &identity.agent_id)?;
        if !claims.iter().any(|claim| {
            claim.name == attempt.name
                && ((claim.kind == "legacy" && claim.owner == attempt.id)
                    || (claim.kind == "agent" && claim.owner == identity.agent_id))
        }) {
            return Err("Bound deletion does not own the reserved name".into());
        }
        let binding = Binding {
            roster: identity.clone(),
            database,
            runtime_generation: generation,
            profile: profile.clone(),
            claims,
            closure: None,
        };
        fence()?;
        let changed = tx.execute(
            "UPDATE legacy_name_attempts SET removal_state = 'intent', removal_binding_json = ?3
            WHERE attempt_id = ?1 AND sequence = ?2 AND removal_state IS NULL",
            params![
                attempt.id,
                attempt.sequence,
                serde_json::to_string(&binding)?
            ],
        )?;
        if changed != 1 {
            return Err("Bound deletion lost its intent fence".into());
        }
        let result = BoundRemovalTicket {
            agent_id: identity.agent_id.clone(),
            attempt_id: attempt.id,
            sequence: attempt.sequence,
            state: "intent".into(),
            binding,
        };
        tx.commit()?;
        Ok(Some(result))
    })
    .map_err(|error| error.to_string())
}

/// Read-only classification under the caller's lifecycle and roster guards.
/// A failed query must never be interpreted as ordinary removal.
pub fn requires_bound_removal(agent: &str) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| {
        let Some(attempt) = latest(conn, agent)? else {
            return Ok(false);
        };
        reject_older_pending(conn, agent, attempt.sequence)?;
        Ok(attempt.state.is_some()
            || matches!(
                attempt.phase.as_str(),
                "reserved" | "effects_possible" | "completed"
            ))
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn reject_bound_ordinary_delete(conn: &Connection, agent: &str) -> StoreResult<()> {
    if let Some(attempt) = latest(conn, agent)? {
        reject_older_pending(conn, agent, attempt.sequence)?;
        if attempt.state.is_some()
            || matches!(
                attempt.phase.as_str(),
                "reserved" | "effects_possible" | "completed"
            )
        {
            return Err("Ordinary deletion acquired pending bound ownership".into());
        }
    }
    Ok(())
}

/// Resolve only the latest exact ticket, including terminal rows in its fence.
pub fn bound_removal_ticket(agent: &str) -> Result<Option<BoundRemovalTicket>, String> {
    crate::db::get_db_conn(|conn| {
        let Some(attempt) = latest(conn, agent)? else {
            return Ok(None);
        };
        reject_older_pending(conn, agent, attempt.sequence)?;
        if attempt.state.is_none() {
            return Ok(None);
        }
        Ok(Some(ticket(agent, attempt)?))
    })
    .map_err(|error| error.to_string())
}

fn current_ticket(
    conn: &Connection,
    expected: &BoundRemovalTicket,
) -> StoreResult<(BoundRemovalTicket, String, String)> {
    let row = latest(conn, &expected.agent_id)?.ok_or("Deletion ticket disappeared")?;
    if row.sequence != expected.sequence || row.id != expected.attempt_id {
        return Err("Deletion ticket is no longer the latest attempt".into());
    }
    let purpose = row.purpose.clone();
    let phase = row.phase.clone();
    let current = ticket(&expected.agent_id, row)?;
    let mut expected_binding = expected.binding.clone();
    expected_binding.closure = current.binding.closure.clone();
    if expected_binding != current.binding {
        return Err("Deletion binding changed".into());
    }
    Ok((current, purpose, phase))
}

fn unchanged_store(conn: &Connection, binding: &Binding) -> StoreResult<()> {
    let actual = crate::db::get_agent_by_session_id_with_conn(conn, &binding.roster.agent_id)?
        .map(DbIdentity::from);
    if actual.as_ref() != Some(&binding.database)
        || claims(conn, &binding.roster.agent_id)? != binding.claims
    {
        return Err("Deletion database or name ownership changed".into());
    }
    Ok(())
}

/// Seal physical closure separately so a later DELETE abort cannot erase it.
/// Recheck the retained lease after waiting for the SQL transaction.
pub fn seal_bound_removal(
    expected: &BoundRemovalTicket,
    closure: &RemovalClosureReceipt,
    profile: &RemovalProfileIdentity,
    fence: impl FnOnce() -> Result<(), String>,
) -> Result<BoundRemovalTicket, String> {
    if !closure.broker_closed
        || !closure.all_process_handles_joined
        || closure.runtime_generation != expected.binding.runtime_generation
    {
        return Err("Bound deletion has no exact owned closure proof".into());
    }
    crate::db::get_db_conn(|conn| {
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let (mut current, _, _) = current_ticket(&tx, expected)?;
        if profile != &current.binding.profile { return Err("Deletion profile ownership changed".into()); }
        unchanged_store(&tx, &current.binding)?;
        if current.state != "intent" {
            if current.binding.closure.as_ref() == Some(closure) { tx.commit()?; return Ok(current); }
            return Err("Deletion already has a different closure receipt".into());
        }
        current.binding.closure = Some(closure.clone());
        fence()?;
        let sealed = tx.execute("UPDATE legacy_name_attempts SET removal_state = 'closure_proved', removal_binding_json = ?3
            WHERE attempt_id = ?1 AND sequence = ?2 AND removal_state = 'intent'",
            params![current.attempt_id, current.sequence, serde_json::to_string(&current.binding)?])?;
        if sealed != 1 { return Err("Deletion lost its closure seal fence".into()); }
        current.state = "closure_proved".into();
        tx.commit()?;
        Ok(current)
    }).map_err(|error| error.to_string())
}

/// Complete only metadata under fresh live or exclusive-home ownership. The
/// caller supplies the actual JSON and current profile under the roster barrier.
/// The final lease fence runs in the transaction immediately before deletion.
/// Lifecycle and roster guards remain with the caller through this callback.
pub fn complete_bound_removal(
    expected: &BoundRemovalTicket,
    roster: &[crate::models::AgentConfig],
    profile: &RemovalProfileIdentity,
    fence: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if roster
        .iter()
        .any(|config| config.session_id == expected.agent_id)
    {
        return Err("Deletion roster contains a target incarnation".into());
    }
    crate::db::get_db_conn(|conn| {
        let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        let (current, purpose, phase) = current_ticket(&tx, expected)?;
        if current.state == "done" {
            if crate::db::get_agent_by_session_id_with_conn(&tx, &current.agent_id)?.is_some() {
                return Err("Completed deletion has a replacement database row".into());
            }
            if profile.directory_id.is_some() && profile != &current.binding.profile {
                return Err("Completed deletion has a replacement profile".into());
            }
            tx.commit()?;
            return Ok(());
        }
        if current.state != "closure_proved" || current.binding.closure.as_ref().is_none_or(|proof|
            !proof.broker_closed || !proof.all_process_handles_joined) {
            return Err("Deletion closure remains unproved".into());
        }
        if profile != &current.binding.profile { return Err("Deletion profile ownership changed".into()); }
        unchanged_store(&tx, &current.binding)?;
        let uncertain: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_spawn_operations
            WHERE planned_agent_id = ?1 AND state IN ('queued','preparing','launching','outcome_unknown'))
            OR EXISTS(SELECT 1 FROM legacy_name_attempts WHERE agent_id = ?1 AND purpose != 'rename'
                AND phase IN ('reserved','effects_possible'))", params![current.agent_id], |row| row.get(0))?;
        if uncertain || !(phase == "completed" || (purpose == "rename" && matches!(phase.as_str(), "reserved" | "effects_possible"))) {
            return Err("Deletion cannot cover unresolved launch ownership".into());
        }
        fence()?;
        super::delete_removed_agent_claims(&tx, &current.agent_id)?;
        crate::db::delete_agent_records(&tx, &current.agent_id)?;
        let completed = tx.execute("UPDATE legacy_name_attempts SET phase = 'removed', removal_state = 'done'
            WHERE attempt_id = ?1 AND sequence = ?2", params![current.attempt_id, current.sequence])?;
        if completed != 1 { return Err("Deletion lost its completion fence".into()); }
        tx.commit()?;
        Ok(())
    }).map_err(|error| error.to_string())
}

/// Startup inspects dispositions without adopting names or recreating agents.
pub fn pending_bound_removals() -> Result<Vec<BoundRemovalTicket>, String> {
    crate::db::get_db_conn(|conn| {
        let mut query = conn.prepare(
            "SELECT DISTINCT agent_id FROM legacy_name_attempts
            WHERE removal_state IN ('intent','closure_proved')",
        )?;
        let agents = query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        agents
            .into_iter()
            .map(|agent| {
                let row = latest(conn, &agent)?.ok_or("Deletion attempt is missing")?;
                reject_older_pending(conn, &agent, row.sequence)?;
                ticket(&agent, row)
            })
            .collect()
    })
    .map_err(|error| error.to_string())
}
