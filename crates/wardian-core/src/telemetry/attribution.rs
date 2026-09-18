//! Verified parent attribution for telemetry read projections.
//!
//! Provider child records are linked to a root only through the durable worker
//! record and the provider stored on the fact. The roster join keeps a foreign
//! or deleted root from manufacturing a row that cannot be opened.

use crate::telemetry::matrix::Measure;
use crate::telemetry::models::TokenCounts;
use rusqlite::{params, Connection};

/// SQL join shared by grouped telemetry queries.
///
/// The worker table is intentionally not state-filtered. Historical Analytics
/// must keep a verified mapping after a child leaves the active roster or its
/// detail retention expires. `HAVING COUNT(*) = 1` fails closed if malformed
/// data ever provides more than one candidate for the same fact identity.
pub(crate) fn agent_root_join(alias: &str) -> String {
    format!(
        "LEFT JOIN (
             SELECT worker_id, provider, MAX(root_agent_id) AS root_agent_id
             FROM temporary_workers
             WHERE kind = 'provider_child'
             GROUP BY worker_id, provider
             HAVING COUNT(*) = 1 AND MAX(root_agent_id) IS NOT NULL
         ) worker_root
           ON worker_root.worker_id = {alias}.session_id
          AND worker_root.provider = {alias}.provider
         LEFT JOIN agents root_agent
           ON root_agent.session_id = worker_root.root_agent_id"
    )
}

/// The canonical grouped key for one fact-table alias.
pub(crate) fn agent_root_key(alias: &str) -> String {
    format!("COALESCE(root_agent.session_id, {alias}.session_id)")
}

/// Grouped agent keys with any reported token component in an exact window.
///
/// Query facts once for the whole fleet, using the same durable root projection
/// as its totals. Zero is reported accounting; only five NULL components mean
/// unreported. Hourly rollups cannot establish presence for mid-hour windows.
pub fn token_reporting_agents(
    conn: &Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<std::collections::HashSet<String>> {
    let key = agent_root_key("t");
    let join = agent_root_join("t");
    let mut statement = conn.prepare(&format!(
        "SELECT DISTINCT {key} FROM telemetry_turns t {join}
         WHERE t.ended_at >= ?1 AND t.ended_at < ?2
           AND (t.input_tokens IS NOT NULL
                OR t.cached_input_tokens IS NOT NULL
                OR t.cache_write_tokens IS NOT NULL
                OR t.output_tokens IS NOT NULL
                OR t.reasoning_tokens IS NOT NULL)"
    ))?;
    let rows = statement.query_map(params![from, to], |row| row.get::<_, String>(0))?;
    rows.collect()
}

/// Distinct turn identity that cannot collapse equal provider turn IDs from
/// different source sessions. Length prefixes keep arbitrary stored IDs from
/// colliding when they contain the old delimiter character.
pub(crate) fn distinct_turn_expr(alias: &str) -> String {
    format!(
        "COUNT(DISTINCT printf('%d:', length(CAST({alias}.provider AS BLOB))) || hex(CAST({alias}.provider AS BLOB)) || printf('%d:', length(CAST({alias}.session_id AS BLOB))) || hex(CAST({alias}.session_id AS BLOB)) || printf('%d:', length(CAST(COALESCE({alias}.turn_id, {alias}.event_key) AS BLOB))) || hex(CAST(COALESCE({alias}.turn_id, {alias}.event_key) AS BLOB)))"
    )
}

/// Aggregated measures for one attribution partition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentMeasureValues {
    pub active_ms: i64,
    pub turns: i64,
    pub tokens: TokenCounts,
    pub files: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
}

impl AgentMeasureValues {
    /// Return the value in the same stable order as the telemetry measure enum.
    pub fn value(self, measure: Measure) -> Option<i64> {
        match measure {
            Measure::ActiveMs => Some(self.active_ms),
            Measure::Turns => Some(self.turns),
            Measure::FreshTokens => self.tokens.input_tokens,
            Measure::CachedTokens => self.tokens.cached_input_tokens,
            Measure::CacheWriteTokens => self.tokens.cache_write_tokens,
            Measure::OutputTokens => self.tokens.output_tokens,
            Measure::ReasoningTokens => self.tokens.reasoning_tokens,
            Measure::TotalTokens => self.tokens.processed_total(),
            Measure::CacheHitRate => self.cache_hit_rate(),
            Measure::Files => Some(self.files),
            Measure::LinesAdded => Some(self.lines_added),
            Measure::LinesRemoved => Some(self.lines_removed),
            Measure::LinesChanged => Some(self.lines_added + self.lines_removed),
        }
    }

    fn cache_hit_rate(self) -> Option<i64> {
        if self.tokens.input_tokens.is_none()
            && self.tokens.cache_write_tokens.is_none()
            && self.tokens.cached_input_tokens.is_none()
        {
            return None;
        }
        let cached = self.tokens.cached_input_tokens.unwrap_or(0);
        let fresh = self.tokens.input_tokens.unwrap_or(0);
        let writes = self.tokens.cache_write_tokens.unwrap_or(0);
        let denominator = fresh + writes + cached;
        Some(if denominator == 0 {
            0
        } else {
            100 * cached / denominator
        })
    }
}

/// Own work, verified descendants, and the combined answer for one root.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentBreakdown {
    pub total: AgentMeasureValues,
    pub own: AgentMeasureValues,
    pub subagents: AgentMeasureValues,
}

/// Aggregate raw immutable facts for one roster root.
///
/// The own and subagent partitions are computed independently. Distinct turns
/// and files are re-counted over the combined fact set, and cache hit rate is
/// derived from combined token components, so the combined row is not a sum of
/// rendered partitions for non-additive measures.
pub fn agent_breakdown(
    conn: &Connection,
    session_id: &str,
    from: &str,
    to: &str,
) -> rusqlite::Result<AgentBreakdown> {
    let mut answer = AgentBreakdown::default();
    load_turns(conn, session_id, from, to, &mut answer)?;
    load_edits(conn, session_id, from, to, &mut answer)?;
    load_activity(conn, session_id, from, to, &mut answer)?;
    Ok(answer)
}

fn load_turns(
    conn: &Connection,
    session_id: &str,
    from: &str,
    to: &str,
    answer: &mut AgentBreakdown,
) -> rusqlite::Result<()> {
    let key = agent_root_key("t");
    let join = agent_root_join("t");
    let distinct_turns = distinct_turn_expr("t");
    let sql = format!(
        "SELECT CASE WHEN t.session_id = ?3 THEN 0 ELSE 1 END,
                {distinct_turns},
                SUM(t.input_tokens), SUM(t.cached_input_tokens),
                SUM(t.cache_write_tokens), SUM(t.output_tokens),
                SUM(t.reasoning_tokens)
         FROM telemetry_turns t {join}
         WHERE t.ended_at >= ?1 AND t.ended_at < ?2
           AND (t.session_id = ?3 OR {key} = ?3)
         GROUP BY CASE WHEN t.session_id = ?3 THEN 0 ELSE 1 END"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params![from, to, session_id], |row| {
        Ok((
            row.get::<_, i64>(0)? as usize,
            AgentMeasureValues {
                turns: row.get(1)?,
                tokens: TokenCounts {
                    input_tokens: row.get(2)?,
                    cached_input_tokens: row.get(3)?,
                    cache_write_tokens: row.get(4)?,
                    output_tokens: row.get(5)?,
                    reasoning_tokens: row.get(6)?,
                },
                ..Default::default()
            },
        ))
    })?;
    for row in rows {
        let (partition, values) = row?;
        if partition == 0 {
            answer.own.turns = values.turns;
            answer.own.tokens = values.tokens;
        } else {
            answer.subagents.turns = values.turns;
            answer.subagents.tokens = values.tokens;
        }
    }

    let total_sql = format!(
        "SELECT {distinct_turns}
         FROM telemetry_turns t {join}
         WHERE t.ended_at >= ?1 AND t.ended_at < ?2
           AND (t.session_id = ?3 OR {key} = ?3)"
    );
    answer.total.turns =
        conn.query_row(&total_sql, params![from, to, session_id], |row| row.get(0))?;
    answer.total.tokens = answer.own.tokens;
    answer.total.tokens.add(&answer.subagents.tokens);
    Ok(())
}

fn load_edits(
    conn: &Connection,
    session_id: &str,
    from: &str,
    to: &str,
    answer: &mut AgentBreakdown,
) -> rusqlite::Result<()> {
    let key = agent_root_key("e");
    let join = agent_root_join("e");
    let sql = format!(
        "SELECT CASE WHEN e.session_id = ?3 THEN 0 ELSE 1 END,
                COUNT(DISTINCT e.path),
                COALESCE(SUM(e.lines_added), 0),
                COALESCE(SUM(e.lines_removed), 0)
         FROM telemetry_edits e {join}
         WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2
           AND (e.session_id = ?3 OR {key} = ?3)
         GROUP BY CASE WHEN e.session_id = ?3 THEN 0 ELSE 1 END"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params![from, to, session_id], |row| {
        Ok((
            row.get::<_, i64>(0)? as usize,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (partition, files, lines_added, lines_removed) = row?;
        let target = if partition == 0 {
            &mut answer.own
        } else {
            &mut answer.subagents
        };
        target.files = files;
        target.lines_added = lines_added;
        target.lines_removed = lines_removed;
    }

    let total_sql = format!(
        "SELECT COUNT(DISTINCT e.path),
                COALESCE(SUM(e.lines_added), 0),
                COALESCE(SUM(e.lines_removed), 0)
         FROM telemetry_edits e {join}
         WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2
           AND (e.session_id = ?3 OR {key} = ?3)"
    );
    let (files, lines_added, lines_removed) =
        conn.query_row(&total_sql, params![from, to, session_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    answer.total.files = files;
    answer.total.lines_added = lines_added;
    answer.total.lines_removed = lines_removed;
    Ok(())
}

fn load_activity(
    conn: &Connection,
    session_id: &str,
    from: &str,
    to: &str,
    answer: &mut AgentBreakdown,
) -> rusqlite::Result<()> {
    let key = agent_root_key("a");
    let join = agent_root_join("a");
    let sql = format!(
        "SELECT CASE WHEN a.session_id = ?3 THEN 0 ELSE 1 END,
                COALESCE(SUM(
                  (julianday(MIN(a.ended_at, ?2))
                   - julianday(MAX(a.started_at, ?1))) * 86400000.0
                ), 0)
         FROM telemetry_activity a {join}
         WHERE a.ended_at > ?1 AND a.started_at < ?2
           AND (a.session_id = ?3 OR {key} = ?3)
         GROUP BY CASE WHEN a.session_id = ?3 THEN 0 ELSE 1 END"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params![from, to, session_id], |row| {
        Ok((row.get::<_, i64>(0)? as usize, row.get::<_, f64>(1)? as i64))
    })?;
    for row in rows {
        let (partition, active_ms) = row?;
        if partition == 0 {
            answer.own.active_ms = active_ms;
        } else {
            answer.subagents.active_ms = active_ms;
        }
    }
    answer.total.active_ms = answer.own.active_ms + answer.subagents.active_ms;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::horizon::HorizonWindow;
    use crate::telemetry::matrix::{grouped_matrix_at, grouped_totals_at, matrix_at, Measure};
    use crate::telemetry::query::{grouped_breakdown, Dimension};
    use crate::telemetry::schema::run_telemetry_migrations;
    use crate::temporary_workers;

    const FROM: &str = "2026-09-18T00:00:00.000Z";
    const TO: &str = "2026-09-18T02:00:00.000Z";
    const EVERY_MEASURE: [Measure; 13] = [
        Measure::ActiveMs,
        Measure::Turns,
        Measure::FreshTokens,
        Measure::CachedTokens,
        Measure::CacheWriteTokens,
        Measure::OutputTokens,
        Measure::ReasoningTokens,
        Measure::TotalTokens,
        Measure::CacheHitRate,
        Measure::Files,
        Measure::LinesAdded,
        Measure::LinesRemoved,
        Measure::LinesChanged,
    ];

    fn window() -> HorizonWindow {
        HorizonWindow {
            from: FROM.into(),
            to: TO.into(),
            from_floored: false,
        }
    }

    fn telemetry_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_telemetry_migrations(&conn).unwrap();
        conn
    }

    fn grouped_db() -> Connection {
        let conn = telemetry_db();
        conn.execute_batch(
            "CREATE TABLE agents (
                 session_id TEXT PRIMARY KEY,
                 session_name TEXT
             );",
        )
        .unwrap();
        temporary_workers::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO agents(session_id, session_name) VALUES ('root', 'Root agent')",
            [],
        )
        .unwrap();
        conn
    }

    fn worker(conn: &Connection, worker_id: &str, provider: &str, root_agent_id: &str) {
        conn.execute(
            "INSERT INTO temporary_workers (
                 worker_id, kind, provider, workspace, root_agent_id,
                 runtime_session_id, state, capabilities_json, coverage,
                 requested_at, last_observed_at, detail_retained_until
             ) VALUES (?1, 'provider_child', ?2, 'workspace', ?3, ?1,
                       'completed', '{}', 'verified', ?4, ?4, '2026-01-01T00:00:00Z')",
            params![worker_id, provider, root_agent_id, FROM],
        )
        .unwrap();
    }

    struct TokenFixture {
        input: Option<i64>,
        cached: Option<i64>,
        output: Option<i64>,
    }

    fn turn(
        conn: &Connection,
        event_key: &str,
        session_id: &str,
        provider: &str,
        turn_id: &str,
        tokens: TokenFixture,
    ) {
        conn.execute(
            "INSERT INTO telemetry_turns (
                 event_key, session_id, provider, turn_id, ended_at,
                 input_tokens, cached_input_tokens, cache_write_tokens,
                 output_tokens, reasoning_tokens, source_key, source_path
             ) VALUES (?1, ?2, ?3, ?4, '2026-09-18T00:30:00.000Z',
                       ?5, ?6, 0, ?7, ?7, ?8, 'telemetry')",
            params![
                event_key,
                session_id,
                provider,
                turn_id,
                tokens.input,
                tokens.cached,
                tokens.output,
                format!("source-{event_key}"),
            ],
        )
        .unwrap();
    }

    fn edit(conn: &Connection, event_key: &str, session_id: &str, path: &str, lines: i64) {
        conn.execute(
            "INSERT INTO telemetry_edits (
                 event_key, session_id, provider, turn_id, occurred_at,
                 path, op, lines_added, lines_removed, source_key, source_path
             ) VALUES (?1, ?2, 'codex', NULL, '2026-09-18T00:30:00.000Z',
                       ?3, 'modify', ?4, 0, ?5, 'telemetry')",
            params![
                event_key,
                session_id,
                path,
                lines,
                format!("source-{event_key}")
            ],
        )
        .unwrap();
    }

    fn activity(conn: &Connection, event_key: &str, session_id: &str, started: &str, ended: &str) {
        conn.execute(
            "INSERT INTO telemetry_activity (
                 session_id, provider, started_at, ended_at, last_event_at,
                 event_count, method, source_key
             ) VALUES (?1, 'codex', ?2, ?3, ?3, 1, 'measured', ?4)",
            params![session_id, started, ended, format!("source-{event_key}")],
        )
        .unwrap();
    }

    #[test]
    fn fleet_reporting_uses_grouped_keys_and_all_token_components() {
        for component in [
            "input_tokens",
            "cached_input_tokens",
            "cache_write_tokens",
            "output_tokens",
            "reasoning_tokens",
        ] {
            let conn = grouped_db();
            worker(&conn, "child", "codex", "root");
            worker(&conn, "foreign", "claude", "root");
            worker(&conn, "silent", "codex", "root");
            // No parent fact. Every component is tested alone, with the other
            // four remaining NULL (including cache writes).
            let insert = |session: &str, value: Option<i64>, at: &str| {
                conn.execute(
                    &format!(
                        "INSERT INTO telemetry_turns
                         (event_key, session_id, provider, ended_at,
                          {component}, source_key, source_path)
                         VALUES (?1, ?1, 'codex', ?2, ?3, ?1, 'telemetry')"
                    ),
                    params![session, at, value],
                )
                .unwrap();
            };
            let at = "2026-09-18T00:30:00.000Z";
            insert("foreign", Some(99), at);
            insert("silent", None, at);
            insert("direct", Some(0), at);
            insert("at-upper-bound", Some(99), TO);
            let from = "2026-09-18T00:15:00.000Z";
            let reported = token_reporting_agents(&conn, from, TO).unwrap();
            assert_eq!(
                reported,
                ["foreign".to_string(), "direct".to_string()].into(),
                "{component}: foreign or unreported child must not mark root"
            );

            insert("child", Some(7), at);
            let reported = token_reporting_agents(&conn, from, TO).unwrap();
            assert_eq!(
                reported,
                [
                    "root".to_string(),
                    "foreign".to_string(),
                    "direct".to_string()
                ]
                .into(),
                "{component}: child reporting must project to root"
            );
            let exact_window = HorizonWindow {
                from: from.into(),
                to: TO.into(),
                from_floored: false,
            };
            let totals = grouped_totals_at(
                &conn,
                &exact_window,
                Dimension::Agent,
                &[Measure::TotalTokens],
            )
            .unwrap();
            // Mirror the fleet's presence gate: child-only spend stays visible.
            // Cached/reasoning-only accounting reports a true processed zero.
            let visible = reported
                .contains("root")
                .then_some(totals[&Measure::TotalTokens]["root"]);
            let expected = if matches!(component, "cached_input_tokens" | "reasoning_tokens") {
                0
            } else {
                7
            };
            assert_eq!(visible, Some(expected), "{component}");
        }
    }

    #[test]
    fn historical_children_stay_attributed_after_detail_expiry() {
        let conn = grouped_db();
        worker(&conn, "child", "codex", "root");
        worker(&conn, "nested", "codex", "root");
        worker(&conn, "foreign", "codex", "deleted-root");
        worker(&conn, "mismatch", "claude", "root");

        turn(
            &conn,
            "root-event",
            "root",
            "codex",
            "same-turn",
            TokenFixture {
                input: Some(100),
                cached: Some(900),
                output: Some(10),
            },
        );
        turn(
            &conn,
            "child-event",
            "child",
            "codex",
            "same-turn",
            TokenFixture {
                input: Some(100),
                cached: Some(0),
                output: Some(10),
            },
        );
        turn(
            &conn,
            "nested-event",
            "nested",
            "codex",
            "nested-turn",
            TokenFixture {
                input: None,
                cached: None,
                output: None,
            },
        );
        turn(
            &conn,
            "foreign-event",
            "foreign",
            "codex",
            "foreign-turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );
        turn(
            &conn,
            "mismatch-event",
            "mismatch",
            "codex",
            "mismatch-turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );
        turn(
            &conn,
            "missing-event",
            "missing",
            "codex",
            "missing-turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );
        edit(&conn, "root-edit", "root", "src/shared.rs", 1);
        edit(&conn, "child-edit", "child", "src/shared.rs", 2);
        edit(&conn, "nested-edit", "nested", "src/nested.rs", 3);
        activity(
            &conn,
            "root-activity",
            "root",
            "2026-09-18T00:00:00Z",
            "2026-09-18T00:10:00Z",
        );
        activity(
            &conn,
            "child-activity",
            "child",
            "2026-09-18T00:10:00Z",
            "2026-09-18T00:30:00Z",
        );

        let breakdown = agent_breakdown(&conn, "root", FROM, TO).unwrap();
        assert_eq!(breakdown.own.turns, 1);
        assert_eq!(breakdown.subagents.turns, 2);
        assert_eq!(breakdown.total.turns, 3);
        assert_eq!(breakdown.own.files, 1);
        assert_eq!(breakdown.subagents.files, 2);
        assert_eq!(breakdown.total.files, 2);
        assert_eq!(breakdown.total.lines_added, 6);
        assert_eq!(breakdown.own.value(Measure::CacheHitRate), Some(90));
        assert_eq!(breakdown.subagents.value(Measure::CacheHitRate), Some(0));
        assert_eq!(breakdown.total.value(Measure::CacheHitRate), Some(81));
        assert_eq!(breakdown.total.tokens.processed_total(), Some(220));
        assert!(breakdown.total.active_ms > breakdown.own.active_ms);

        for measure in EVERY_MEASURE {
            let expected = breakdown.total.value(measure).unwrap_or(0);
            let matrix_total =
                grouped_matrix_at(&conn, &window(), Dimension::Agent, measure, 10, None)
                    .unwrap()
                    .rows
                    .into_iter()
                    .find(|row| row.key == "root")
                    .unwrap()
                    .total;
            let total = grouped_totals_at(&conn, &window(), Dimension::Agent, &[measure])
                .unwrap()
                .remove(&measure)
                .and_then(|rows| rows.get("root").copied())
                .unwrap_or(0);
            assert_eq!(
                matrix_total, expected,
                "matrix total mismatch for {measure:?}"
            );
            assert_eq!(total, expected, "window total mismatch for {measure:?}");
        }

        let matrix =
            grouped_matrix_at(&conn, &window(), Dimension::Agent, Measure::Turns, 10, None)
                .unwrap();
        assert_eq!(
            matrix
                .rows
                .iter()
                .find(|row| row.key == "root")
                .unwrap()
                .total,
            3
        );
        assert!(matrix.rows.iter().any(|row| row.key == "foreign"));
        assert!(matrix.rows.iter().any(|row| row.key == "mismatch"));
        assert!(matrix.rows.iter().any(|row| row.key == "missing"));
    }

    #[test]
    fn grouped_row_cap_follows_root_projection_and_direct_reads_need_no_worker_schema() {
        let direct = telemetry_db();
        turn(
            &direct,
            "direct-event",
            "direct",
            "codex",
            "direct-turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );
        let direct_matrix = matrix_at(
            &direct,
            &window(),
            Dimension::Agent,
            Measure::Turns,
            10,
            None,
        )
        .unwrap();
        assert_eq!(direct_matrix.rows[0].key, "direct");

        let conn = grouped_db();
        worker(&conn, "child", "codex", "root");
        turn(
            &conn,
            "root-event",
            "root",
            "codex",
            "root-turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );
        turn(
            &conn,
            "child-event",
            "child",
            "codex",
            "child-turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );
        turn(
            &conn,
            "other-event",
            "other",
            "codex",
            "other-turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );

        let matrix =
            grouped_matrix_at(&conn, &window(), Dimension::Agent, Measure::Turns, 1, None).unwrap();
        assert_eq!(matrix.rows.len(), 1);
        assert_eq!(matrix.rows[0].key, "root");
        assert_eq!(matrix.rows[0].total, 2);
    }

    #[test]
    fn grouped_provider_and_model_reads_keep_their_existing_dimensions() {
        let conn = telemetry_db();
        conn.execute(
            "INSERT INTO telemetry_rollup_hourly (
                 bucket_start, session_id, provider, model,
                 measured_active_ms, input_tokens, output_tokens
             ) VALUES ('2026-09-18T00:00:00.000Z', 'agent', 'codex', 'model-a', 5, 2, 3)",
            [],
        )
        .unwrap();

        let providers = grouped_breakdown(&conn, Dimension::Provider, FROM, TO, 10).unwrap();
        assert_eq!(
            providers
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            ["codex"]
        );
        let models = grouped_breakdown(&conn, Dimension::Model, FROM, TO, 10).unwrap();
        assert_eq!(
            models
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            ["model-a"]
        );
    }

    #[test]
    fn distinct_turn_identity_survives_delimiter_characters() {
        let conn = telemetry_db();
        turn(
            &conn,
            "delimiter-a",
            "b\u{1f}c",
            "a",
            "turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );
        turn(
            &conn,
            "delimiter-b",
            "c",
            "a\u{1f}b",
            "turn",
            TokenFixture {
                input: Some(1),
                cached: None,
                output: Some(1),
            },
        );
        let expression = distinct_turn_expr("t");
        let count: i64 = conn
            .query_row(
                &format!("SELECT {expression} FROM telemetry_turns t"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }
}
