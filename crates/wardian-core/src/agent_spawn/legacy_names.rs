//! Private durable association for the existing spawn, clone and rename adapters.

use crate::models::AgentConfig;
use rusqlite::{params, Connection, OptionalExtension};

/// Existing adapter whose exact name reservation is being durably associated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyNamePurpose {
    Spawn,
    Clone,
    Rename,
}

impl LegacyNamePurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Clone => "clone",
            Self::Rename => "rename",
        }
    }
}

/// Distinguishes a successful metadata retry from a foreign or stale attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyNameFinalization {
    Committed,
    AlreadyCommitted,
    Conflict,
}

pub(super) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS legacy_name_attempts (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            attempt_id TEXT NOT NULL UNIQUE,
            agent_id TEXT NOT NULL,
            purpose TEXT NOT NULL CHECK (purpose IN ('spawn', 'clone', 'rename')),
            previous_name TEXT,
            new_name TEXT NOT NULL,
            phase TEXT NOT NULL CHECK (phase IN (
                'reserved', 'effects_possible', 'completed', 'committed', 'aborted', 'removed'
            ))
        );
        CREATE INDEX IF NOT EXISTS idx_legacy_name_agent
        ON legacy_name_attempts(agent_id, sequence);",
    )?;
    for column in ["removal_state", "removal_binding_json"] {
        let exists = conn
            .prepare("PRAGMA table_info(legacy_name_attempts)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|name| name == column);
        if !exists {
            conn.execute_batch(&format!(
                "ALTER TABLE legacy_name_attempts ADD COLUMN {column} TEXT"
            ))?;
        }
    }
    Ok(())
}

/// Reserve the name and bind its exact attempt before any effect. A failed
/// transaction leaves neither record, including a cut between the two inserts.
pub fn reserve_legacy_name(
    attempt_id: &str,
    agent_id: &str,
    purpose: LegacyNamePurpose,
    previous_name: Option<&str>,
    new_name: &str,
) -> Result<(), String> {
    super::canonical_uuid(attempt_id, "name attempt")?;
    // Renames preserve existing saved identities, including older opaque IDs.
    // New spawn/clone plans continue to require their generated UUID identity.
    if purpose != LegacyNamePurpose::Rename {
        super::canonical_uuid(agent_id, "planned agent")?;
    }
    crate::db::get_db_conn(|conn| {
        reserve_with_conn(conn, attempt_id, agent_id, purpose, previous_name, new_name)
    })
    .map_err(|error| {
        if error.to_string().to_ascii_lowercase().contains("unique") {
            format!("An agent with the name '{new_name}' already exists.")
        } else {
            error.to_string()
        }
    })
}

fn reserve_with_conn(
    conn: &Connection,
    attempt_id: &str,
    agent_id: &str,
    purpose: LegacyNamePurpose,
    previous_name: Option<&str>,
    new_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if (purpose == LegacyNamePurpose::Rename) != previous_name.is_some() {
        return Err("invalid legacy name purpose".into());
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let occupied: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE session_name = ?1
         AND (?2 != 'rename' OR session_id != ?3))",
        params![new_name, purpose.as_str(), agent_id],
        |row| row.get(0),
    )?;
    let pending: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM legacy_name_attempts WHERE agent_id = ?1
         AND (phase IN ('reserved', 'effects_possible', 'completed')
             OR removal_state IN ('intent', 'closure_proved')))",
        params![agent_id],
        |row| row.get(0),
    )?;
    if occupied {
        return Err(format!("An agent with the name '{new_name}' already exists.").into());
    }
    if pending {
        return Err("legacy name ownership is pending for this agent".into());
    }
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    tx.execute(
        "INSERT INTO agent_name_claims(session_name, owner_kind, owner_id, created_at)
         VALUES (?1, 'legacy', ?2, ?3)",
        params![new_name, attempt_id, now],
    )?;
    tx.execute(
        "INSERT INTO legacy_name_attempts
         (attempt_id, agent_id, purpose, previous_name, new_name, phase)
         VALUES (?1, ?2, ?3, ?4, ?5, 'reserved')",
        params![
            attempt_id,
            agent_id,
            purpose.as_str(),
            previous_name,
            new_name
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// Persist the uncertainty boundary before bootstrap, registration or rename I/O.
pub fn begin_legacy_name_effects(attempt_id: &str, agent_id: &str) -> Result<(), String> {
    crate::db::get_db_conn(|conn| begin_with_conn(conn, attempt_id, agent_id))
        .map_err(|error| error.to_string())
}

fn begin_with_conn(
    conn: &Connection,
    attempt_id: &str,
    agent_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let changed = conn.execute(
        "UPDATE legacy_name_attempts SET phase = 'effects_possible'
         WHERE attempt_id = ?1 AND agent_id = ?2 AND phase = 'reserved' AND removal_state IS NULL
         AND EXISTS(SELECT 1 FROM agent_name_claims WHERE session_name = new_name
             AND owner_kind = 'legacy' AND owner_id = ?1)",
        params![attempt_id, agent_id],
    )?;
    if changed != 1 {
        return Err("legacy name attempt lost its pre-effect fence".into());
    }
    Ok(())
}

/// The adapter holds the exact incarnation/lifecycle and roster barriers and
/// calls this only after normal JSON publication and required completion.
pub fn complete_legacy_name(
    attempt_id: &str,
    agent_id: &str,
    roster: &[AgentConfig],
) -> Result<(), String> {
    crate::db::get_db_conn(|conn| complete_with_conn(conn, attempt_id, agent_id, roster))
        .map_err(|error| error.to_string())
}

fn complete_with_conn(
    conn: &Connection,
    attempt_id: &str,
    agent_id: &str,
    roster: &[AgentConfig],
) -> Result<(), Box<dyn std::error::Error>> {
    let tx = conn.unchecked_transaction()?;
    let name: String = tx.query_row(
        "SELECT new_name FROM legacy_name_attempts WHERE attempt_id = ?1
         AND agent_id = ?2 AND phase IN ('effects_possible', 'completed', 'committed') AND removal_state IS NULL",
        params![attempt_id, agent_id],
        |row| row.get(0),
    )?;
    if !publication_matches(&tx, roster, agent_id, &name)? {
        return Err("legacy name publication evidence is incomplete".into());
    }
    tx.execute(
        "UPDATE legacy_name_attempts SET phase = 'completed' WHERE attempt_id = ?1
         AND phase = 'effects_possible'",
        params![attempt_id],
    )?;
    tx.commit()?;
    Ok(())
}

fn publication_matches(
    conn: &Connection,
    roster: &[AgentConfig],
    agent_id: &str,
    name: &str,
) -> rusqlite::Result<bool> {
    let matching = roster
        .iter()
        .filter(|config| config.session_id == agent_id || config.session_name == name)
        .collect::<Vec<_>>();
    if matching.len() != 1 || matching[0].session_id != agent_id || matching[0].session_name != name
    {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE session_id = ?1 AND session_name = ?2)",
        params![agent_id, name],
        |row| row.get(0),
    )
}

/// Metadata-only retry. The persisted completion fence and actual roster are
/// both required. Promotion and the previous owned-name release commit together.
pub fn finalize_legacy_name(
    attempt_id: &str,
    agent_id: &str,
    roster: &[AgentConfig],
) -> Result<LegacyNameFinalization, String> {
    crate::db::get_db_conn(|conn| finalize_with_conn(conn, attempt_id, agent_id, roster))
        .map_err(|error| error.to_string())
}

fn finalize_with_conn(
    conn: &Connection,
    attempt_id: &str,
    agent_id: &str,
    roster: &[AgentConfig],
) -> Result<LegacyNameFinalization, Box<dyn std::error::Error>> {
    let tx = conn.unchecked_transaction()?;
    let intent: Option<(String, Option<String>, String)> = tx
        .query_row(
            "SELECT new_name, previous_name, phase FROM legacy_name_attempts
         WHERE attempt_id = ?1 AND agent_id = ?2 AND removal_state IS NULL AND sequence = (
             SELECT MAX(sequence) FROM legacy_name_attempts WHERE agent_id = ?2)",
            params![attempt_id, agent_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((name, previous_name, phase)) = intent else {
        return Ok(LegacyNameFinalization::Conflict);
    };
    if !publication_matches(&tx, roster, agent_id, &name)? {
        return Ok(LegacyNameFinalization::Conflict);
    }
    let owner: Option<(String, String)> = tx
        .query_row(
            "SELECT owner_kind, owner_id FROM agent_name_claims WHERE session_name = ?1",
            params![name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if phase == "committed" && owner == Some(("agent".into(), agent_id.into())) {
        return Ok(LegacyNameFinalization::AlreadyCommitted);
    }
    if phase != "completed" || owner != Some(("legacy".into(), attempt_id.into())) {
        return Ok(LegacyNameFinalization::Conflict);
    }
    tx.execute(
        "UPDATE agent_name_claims SET owner_kind = 'agent', owner_id = ?2
         WHERE session_name = ?1 AND owner_kind = 'legacy' AND owner_id = ?3",
        params![name, agent_id, attempt_id],
    )?;
    if let Some(previous_name) = previous_name {
        tx.execute(
            "DELETE FROM agent_name_claims WHERE session_name = ?1
             AND owner_kind = 'agent' AND owner_id = ?2",
            params![previous_name, agent_id],
        )?;
    }
    tx.execute(
        "UPDATE legacy_name_attempts SET phase = 'committed' WHERE attempt_id = ?1",
        params![attempt_id],
    )?;
    tx.commit()?;
    Ok(LegacyNameFinalization::Committed)
}

pub(super) fn release_unstarted_with_conn(
    conn: &Connection,
    name: &str,
    attempt_id: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let tx = conn.unchecked_transaction()?;
    let possible: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM legacy_name_attempts WHERE attempt_id = ?1
         AND (new_name != ?2 OR phase != 'reserved' OR removal_state IS NOT NULL))",
        params![attempt_id, name],
        |row| row.get(0),
    )?;
    if possible {
        return Ok(false);
    }
    let changed = tx.execute(
        "DELETE FROM agent_name_claims WHERE session_name = ?1
         AND owner_kind = 'legacy' AND owner_id = ?2",
        params![name, attempt_id],
    )?;
    if changed == 1 {
        tx.execute(
            "UPDATE legacy_name_attempts SET phase = 'aborted' WHERE attempt_id = ?1",
            params![attempt_id],
        )?;
    }
    tx.commit()?;
    Ok(changed == 1)
}

/// A failed rename can release its reservation only after both durable stores
/// prove the previous name. An unsuccessful SQLite rollback retains the fence.
pub fn abort_legacy_rename(
    attempt_id: &str,
    agent_id: &str,
    roster: &[AgentConfig],
) -> Result<bool, String> {
    crate::db::get_db_conn(|conn| abort_rename_with_conn(conn, attempt_id, agent_id, roster))
        .map_err(|error| error.to_string())
}

fn abort_rename_with_conn(
    conn: &Connection,
    attempt_id: &str,
    agent_id: &str,
    roster: &[AgentConfig],
) -> Result<bool, Box<dyn std::error::Error>> {
    let tx = conn.unchecked_transaction()?;
    let names: Option<(String, String)> = tx
        .query_row(
            "SELECT previous_name, new_name FROM legacy_name_attempts WHERE attempt_id = ?1
         AND agent_id = ?2 AND purpose = 'rename' AND phase IN ('reserved', 'effects_possible') AND removal_state IS NULL",
            params![attempt_id, agent_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((previous_name, name)) = names else {
        return Ok(false);
    };
    if !publication_matches(&tx, roster, agent_id, &previous_name)? {
        return Ok(false);
    }
    let changed = tx.execute(
        "DELETE FROM agent_name_claims WHERE session_name = ?1
         AND owner_kind = 'legacy' AND owner_id = ?2",
        params![name, attempt_id],
    )?;
    if changed != 1 {
        return Ok(false);
    }
    tx.execute(
        "UPDATE legacy_name_attempts SET phase = 'aborted' WHERE attempt_id = ?1",
        params![attempt_id],
    )?;
    tx.commit()?;
    Ok(true)
}

/// Called under exclusive home ownership before admission and saved restore.
/// Missing/mixed evidence stays bound and excluded. This never starts a provider.
pub fn recover_legacy_names(roster: &[AgentConfig]) -> Result<(), String> {
    crate::db::get_db_conn(|conn| recover_with_conn(conn, roster))
        .map_err(|error| error.to_string())
}

fn recover_with_conn(
    conn: &Connection,
    roster: &[AgentConfig],
) -> Result<(), Box<dyn std::error::Error>> {
    let attempts = {
        let mut statement = conn.prepare(
            "SELECT attempt_id, agent_id, new_name, phase FROM legacy_name_attempts
             WHERE phase IN ('reserved', 'effects_possible', 'completed') AND removal_state IS NULL ORDER BY sequence",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (attempt_id, agent_id, name, phase) in attempts {
        if phase == "completed" {
            let _ = finalize_with_conn(conn, &attempt_id, &agent_id, roster)?;
        } else if phase == "reserved" {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM agents WHERE session_id = ?1)",
                params![agent_id],
                |row| row.get(0),
            )?;
            if !exists && !roster.iter().any(|config| config.session_id == agent_id) {
                release_unstarted_with_conn(conn, &name, &attempt_id)?;
            } else {
                let _ = abort_rename_with_conn(conn, &attempt_id, &agent_id, roster)?;
            }
        }
    }
    Ok(())
}

/// The caller proved exact-incarnation removal, roster absence and joined owned
/// processes. Unproved launches and operation claims retain their owner links.
pub(crate) fn delete_removed_agent_claims(
    conn: &Connection,
    agent_id: &str,
) -> rusqlite::Result<()> {
    let uncertain_operation: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_spawn_operations WHERE planned_agent_id = ?1
         AND state IN ('queued', 'preparing', 'launching', 'outcome_unknown'))",
        params![agent_id],
        |row| row.get(0),
    )?;
    if uncertain_operation {
        return Ok(());
    }
    let covered = {
        let mut statement = conn.prepare(
            "SELECT attempt_id, new_name FROM legacy_name_attempts WHERE agent_id = ?1
             AND (phase IN ('completed', 'committed') OR (purpose = 'rename'
                 AND phase IN ('reserved', 'effects_possible') AND EXISTS(
                     SELECT 1 FROM agents WHERE session_id = ?1
                     AND session_name IN (previous_name, new_name))))",
        )?;
        let rows = statement
            .query_map(params![agent_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (attempt_id, name) in covered {
        conn.execute(
            "DELETE FROM agent_name_claims WHERE session_name = ?1
             AND owner_kind = 'legacy' AND owner_id = ?2",
            params![name, attempt_id],
        )?;
        conn.execute(
            "UPDATE legacy_name_attempts SET phase = 'removed' WHERE attempt_id = ?1",
            params![attempt_id],
        )?;
    }
    conn.execute(
        "DELETE FROM agent_name_claims WHERE owner_kind = 'agent' AND owner_id = ?1",
        params![agent_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(purpose: LegacyNamePurpose) -> (Connection, Vec<AgentConfig>) {
        let conn = super::super::tests::test_db();
        let previous = (purpose == LegacyNamePurpose::Rename).then_some("old");
        if previous.is_some() {
            conn.execute(
                "INSERT INTO agent_name_claims VALUES ('old', 'agent', 'agent', 'now')",
                [],
            )
            .unwrap();
        }
        reserve_with_conn(&conn, "attempt", "agent", purpose, previous, "new").unwrap();
        conn.execute("INSERT INTO agents VALUES ('agent', 'new')", [])
            .unwrap();
        let roster = vec![AgentConfig {
            session_id: "agent".into(),
            session_name: "new".into(),
            ..Default::default()
        }];
        (conn, roster)
    }

    fn claim_owner(conn: &Connection, name: &str) -> Option<(String, String)> {
        conn.query_row(
            "SELECT owner_kind, owner_id FROM agent_name_claims WHERE session_name = ?1",
            params![name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .unwrap()
    }

    #[test]
    fn reservation_cut_rolls_back_both_records_and_reopened_recovery_releases_unstarted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ledger.sqlite");
        let conn = Connection::open(&path).unwrap();
        super::super::tests::initialize_test_db(&conn);
        conn.execute_batch(
            "CREATE TRIGGER cut_binding BEFORE INSERT ON legacy_name_attempts
            BEGIN SELECT RAISE(ABORT, 'cut between claim and binding'); END;",
        )
        .unwrap();
        assert!(reserve_with_conn(
            &conn,
            "attempt",
            "agent",
            LegacyNamePurpose::Spawn,
            None,
            "new"
        )
        .is_err());
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(claim_owner(&conn, "new"), None);
        conn.execute_batch("DROP TRIGGER cut_binding").unwrap();
        reserve_with_conn(
            &conn,
            "attempt",
            "agent",
            LegacyNamePurpose::Spawn,
            None,
            "new",
        )
        .unwrap();
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        recover_with_conn(&conn, &[]).unwrap();
        assert_eq!(claim_owner(&conn, "new"), None);
        assert!(!super::super::restore_is_excluded_with_conn(&conn, "agent").unwrap());
    }

    #[test]
    fn rename_abort_trigger_rolls_back_promotion_and_old_release_then_retry_is_idempotent() {
        let (conn, roster) = fixture(LegacyNamePurpose::Rename);
        begin_with_conn(&conn, "attempt", "agent").unwrap();
        complete_with_conn(&conn, "attempt", "agent", &roster).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER abort_old_release BEFORE DELETE ON agent_name_claims
            WHEN OLD.session_name = 'old' BEGIN SELECT RAISE(ABORT, 'cut after promotion'); END;",
        )
        .unwrap();
        assert!(finalize_with_conn(&conn, "attempt", "agent", &roster).is_err());
        assert_eq!(
            claim_owner(&conn, "new"),
            Some(("legacy".into(), "attempt".into()))
        );
        assert_eq!(
            claim_owner(&conn, "old"),
            Some(("agent".into(), "agent".into()))
        );
        assert!(super::super::restore_is_excluded_with_conn(&conn, "agent").unwrap());
        conn.execute_batch("DROP TRIGGER abort_old_release")
            .unwrap();
        assert_eq!(
            finalize_with_conn(&conn, "attempt", "agent", &roster).unwrap(),
            LegacyNameFinalization::Committed
        );
        assert_eq!(claim_owner(&conn, "old"), None);
        complete_with_conn(&conn, "attempt", "agent", &roster).unwrap();
        assert_eq!(
            finalize_with_conn(&conn, "attempt", "agent", &roster).unwrap(),
            LegacyNameFinalization::AlreadyCommitted
        );
        assert_eq!(
            finalize_with_conn(&conn, "foreign", "agent", &roster).unwrap(),
            LegacyNameFinalization::Conflict
        );
        assert_eq!(
            finalize_with_conn(&conn, "attempt", "foreign", &roster).unwrap(),
            LegacyNameFinalization::Conflict
        );
        conn.execute(
            "UPDATE agent_name_claims SET owner_id = 'foreign' WHERE session_name = 'new'",
            [],
        )
        .unwrap();
        assert_eq!(
            finalize_with_conn(&conn, "attempt", "agent", &roster).unwrap(),
            LegacyNameFinalization::Conflict
        );
    }

    #[test]
    fn completion_requires_actual_roster_and_recovery_does_metadata_only() {
        for purpose in [LegacyNamePurpose::Spawn, LegacyNamePurpose::Clone] {
            let (conn, roster) = fixture(purpose);
            begin_with_conn(&conn, "attempt", "agent").unwrap();
            assert!(complete_with_conn(&conn, "attempt", "agent", &[]).is_err());
            // Coherent DB/JSON without durable completion remains uncertain.
            recover_with_conn(&conn, &roster).unwrap();
            assert_eq!(
                claim_owner(&conn, "new"),
                Some(("legacy".into(), "attempt".into()))
            );
            assert!(super::super::restore_is_excluded_with_conn(&conn, "agent").unwrap());
            complete_with_conn(&conn, "attempt", "agent", &roster).unwrap();
            conn.execute_batch(
                "CREATE TRIGGER abort_promotion BEFORE UPDATE ON agent_name_claims
                BEGIN SELECT RAISE(ABORT, 'promotion unavailable'); END;",
            )
            .unwrap();
            assert!(finalize_with_conn(&conn, "attempt", "agent", &roster).is_err());
            recover_with_conn(&conn, &[]).unwrap();
            assert_eq!(
                claim_owner(&conn, "new"),
                Some(("legacy".into(), "attempt".into()))
            );
            conn.execute_batch("DROP TRIGGER abort_promotion").unwrap();
            recover_with_conn(&conn, &roster).unwrap();
            assert_eq!(
                claim_owner(&conn, "new"),
                Some(("agent".into(), "agent".into()))
            );
            assert!(!super::super::restore_is_excluded_with_conn(&conn, "agent").unwrap());
        }
    }

    #[test]
    fn pre_effect_release_and_possible_effect_quarantine_are_owner_fenced() {
        let (conn, _) = fixture(LegacyNamePurpose::Spawn);
        conn.execute("DELETE FROM agents", []).unwrap();
        assert!(!release_unstarted_with_conn(&conn, "new", "foreign").unwrap());
        recover_with_conn(&conn, &[]).unwrap();
        assert_eq!(claim_owner(&conn, "new"), None);
        let (conn, _) = fixture(LegacyNamePurpose::Clone);
        begin_with_conn(&conn, "attempt", "agent").unwrap();
        conn.execute("DELETE FROM agents", []).unwrap();
        assert!(!release_unstarted_with_conn(&conn, "new", "attempt").unwrap());
        recover_with_conn(&conn, &[]).unwrap();
        assert!(super::super::restore_is_excluded_with_conn(&conn, "agent").unwrap());
        assert!(claim_owner(&conn, "new").is_some());
    }

    #[test]
    fn rename_failed_json_and_failed_db_rollback_keep_bound_claim() {
        let (conn, mut roster) = fixture(LegacyNamePurpose::Rename);
        roster[0].session_name = "old".into();
        begin_with_conn(&conn, "attempt", "agent").unwrap();
        // DB holds new, JSON holds old, so attempted rollback supplies no proof.
        assert!(!abort_rename_with_conn(&conn, "attempt", "agent", &roster).unwrap());
        assert!(claim_owner(&conn, "new").is_some());
        assert!(super::super::restore_is_excluded_with_conn(&conn, "agent").unwrap());
        conn.execute("UPDATE agents SET session_name = 'old'", [])
            .unwrap();
        assert!(abort_rename_with_conn(&conn, "attempt", "agent", &roster).unwrap());
        assert_eq!(claim_owner(&conn, "new"), None);
        assert_eq!(
            claim_owner(&conn, "old"),
            Some(("agent".into(), "agent".into()))
        );
    }

    #[test]
    fn later_attempt_fences_earlier_committed_rename() {
        let (conn, mut roster) = fixture(LegacyNamePurpose::Rename);
        begin_with_conn(&conn, "attempt", "agent").unwrap();
        complete_with_conn(&conn, "attempt", "agent", &roster).unwrap();
        finalize_with_conn(&conn, "attempt", "agent", &roster).unwrap();
        reserve_with_conn(
            &conn,
            "next",
            "agent",
            LegacyNamePurpose::Rename,
            Some("new"),
            "later",
        )
        .unwrap();
        begin_with_conn(&conn, "next", "agent").unwrap();
        conn.execute("UPDATE agents SET session_name = 'later'", [])
            .unwrap();
        roster[0].session_name = "later".into();
        complete_with_conn(&conn, "next", "agent", &roster).unwrap();
        finalize_with_conn(&conn, "next", "agent", &roster).unwrap();
        assert_eq!(
            finalize_with_conn(&conn, "attempt", "agent", &roster).unwrap(),
            LegacyNameFinalization::Conflict
        );
        assert_eq!(
            claim_owner(&conn, "later"),
            Some(("agent".into(), "agent".into()))
        );
    }

    #[test]
    fn recovery_aborts_unstarted_rename_only_with_both_previous_name_proofs() {
        let (conn, mut roster) = fixture(LegacyNamePurpose::Rename);
        roster[0].session_name = "old".into();
        recover_with_conn(&conn, &roster).unwrap();
        assert!(claim_owner(&conn, "new").is_some());
        conn.execute("UPDATE agents SET session_name = 'old'", [])
            .unwrap();
        recover_with_conn(&conn, &roster).unwrap();
        assert_eq!(claim_owner(&conn, "new"), None);
        assert_eq!(
            claim_owner(&conn, "old"),
            Some(("agent".into(), "agent".into()))
        );
        assert!(!super::super::restore_is_excluded_with_conn(&conn, "agent").unwrap());
    }

    #[test]
    fn owned_removal_preserves_operation_unknown_and_all_its_name_links() {
        let (conn, roster) = fixture(LegacyNamePurpose::Spawn);
        begin_with_conn(&conn, "attempt", "agent").unwrap();
        complete_with_conn(&conn, "attempt", "agent", &roster).unwrap();
        conn.execute_batch(
            "INSERT INTO agent_spawn_operations
             (operation_id, payload_hash, request_json, planned_agent_id, session_name,
              provider, class, state, created_at, updated_at)
             VALUES ('op', 'hash', '{}', 'agent', 'operation', 'mock', 'Coder', 'outcome_unknown', 'now', 'now');
             INSERT INTO agent_name_claims VALUES ('operation', 'operation', 'op', 'now');",
        ).unwrap();
        delete_removed_agent_claims(&conn, "agent").unwrap();
        assert_eq!(
            claim_owner(&conn, "new"),
            Some(("legacy".into(), "attempt".into()))
        );
        assert_eq!(
            claim_owner(&conn, "operation"),
            Some(("operation".into(), "op".into()))
        );
        assert!(super::super::restore_is_excluded_with_conn(&conn, "agent").unwrap());
    }
}
