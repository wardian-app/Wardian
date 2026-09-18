//! Read paths for the Dashboard and Analytics surfaces.
//!
//! Additive measures read `telemetry_rollup_hourly`. That is the point of the
//! rollup: a horizon query touches at most one row per hour per agent per
//! model, rather than the tens of thousands of raw events those rows summarize.
//!
//! Three things deliberately do not come from the rollup, each because a rollup
//! cannot represent them:
//!
//! - **Distinct counts** (`turns`, `files_touched`). Distinctness does not
//!   survive pre-aggregation — a per-bucket count answers "how many distinct
//!   values *in this hour*", and no combination of those recovers the global
//!   answer. Summing them counts a file edited across forty hours forty times.
//!   These come from the fact tables for the window, one indexed query each.
//! - **Activity intervals**, because a timeline needs the real spans.
//! - **Rate limits**, which are account-level gauges rather than measures.

use crate::telemetry::attribution::{agent_root_join, agent_root_key, distinct_turn_expr};
use crate::telemetry::models::{
    ActiveTime, ActivityMethod, BreakdownRow, IntervalFact, LimitObservation, TelemetrySummary,
    TokenCounts,
};
use rusqlite::{params, Connection};
use std::collections::HashMap;

/// One bucket of a series.
///
/// Active time stays split by method here as everywhere else, so a chart can
/// stack or separate the two rather than being handed a blended figure it has no
/// way to decompose again.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeriesPoint {
    pub bucket_start: String,
    pub key: String,
    pub active: ActiveTime,
    pub processed_tokens: Option<i64>,
}

/// Grouping dimension for a breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Provider,
    Agent,
    Model,
}

impl Dimension {
    /// The column grouped on. Never built from caller input — see [`parse`].
    ///
    /// [`parse`]: Dimension::parse
    fn column(self) -> &'static str {
        match self {
            Dimension::Provider => "provider",
            Dimension::Agent => "session_id",
            Dimension::Model => "model",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Dimension::Provider => "provider",
            Dimension::Agent => "agent",
            Dimension::Model => "model",
        }
    }

    /// Resolve a caller-supplied name to a dimension.
    ///
    /// This is the only way a string reaches [`column`], and it returns `None`
    /// rather than a default for anything unrecognized. The column is
    /// interpolated into SQL, so an unvalidated name would be an injection
    /// point; keeping the mapping total and closed is what makes the
    /// interpolation safe.
    ///
    /// [`column`]: Dimension::column
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "provider" => Some(Dimension::Provider),
            // Both spellings, because the store calls it `session_id` and the
            // UI calls it an agent.
            "agent" | "session_id" => Some(Dimension::Agent),
            "model" => Some(Dimension::Model),
            _ => None,
        }
    }
}

/// Aggregate measures across a half-open horizon `[from, to)`.
///
/// Durations are COALESCEd because zero active milliseconds is a true statement
/// about an empty horizon. Token sums are not, because zero tokens and no token
/// accounting are different claims and only one of them is safe to display.
pub fn summary(conn: &Connection, from: &str, to: &str) -> rusqlite::Result<TelemetrySummary> {
    // Two measures cannot come from the rollup at all, because they are
    // `COUNT(DISTINCT ...)` and distinctness does not survive pre-aggregation:
    // the stored per-bucket counts sum to file-hours and turn-hours, not files
    // and turns. On one real habitat that read 1,813 files where 967 were
    // touched. Both are answered from the fact tables against the same window,
    // which is one indexed query each and leaves every additive measure on the
    // rollup fast path.
    let files_touched = distinct_count(
        conn,
        "SELECT COUNT(DISTINCT path) FROM telemetry_edits
         WHERE occurred_at >= ?1 AND occurred_at < ?2",
        from,
        to,
    )?;
    let turns = distinct_count(
        conn,
        // Turns without an id are still turns; they simply have no identity to
        // collapse on, so each event counts once.
        "SELECT COUNT(DISTINCT COALESCE(turn_id, event_key)) FROM telemetry_turns
         WHERE ended_at >= ?1 AND ended_at < ?2",
        from,
        to,
    )?;

    conn.query_row(
        "SELECT COALESCE(SUM(measured_active_ms), 0),
                COALESCE(SUM(clustered_active_ms), 0),
                0,
                SUM(input_tokens),
                SUM(cached_input_tokens),
                SUM(cache_write_tokens),
                SUM(output_tokens),
                SUM(reasoning_tokens),
                0,
                COALESCE(SUM(lines_added), 0),
                COALESCE(SUM(lines_removed), 0),
                COUNT(DISTINCT session_id)
         FROM telemetry_rollup_hourly
         WHERE bucket_start >= ?1 AND bucket_start < ?2",
        params![from, to],
        |row| {
            Ok(TelemetrySummary {
                active: ActiveTime {
                    measured_ms: row.get(0)?,
                    clustered_ms: row.get(1)?,
                },
                turns,
                tokens: TokenCounts {
                    input_tokens: row.get(3)?,
                    cached_input_tokens: row.get(4)?,
                    cache_write_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    reasoning_tokens: row.get(7)?,
                },
                files_touched,
                lines_added: row.get(9)?,
                lines_removed: row.get(10)?,
                agent_count: row.get(11)?,
            })
        },
    )
}

/// Distinct turns and distinct files per group, keyed by the dimension's value.
///
/// The grouped counterpart of [`distinct_count`], and needed for the same
/// reason: a breakdown that read these two off the rollup would not reconcile
/// against a summary that does not.
fn distinct_by_dimension(
    conn: &Connection,
    dimension: Dimension,
    from: &str,
    to: &str,
    grouped_agents: bool,
) -> rusqlite::Result<(HashMap<String, i64>, HashMap<String, i64>)> {
    let turns = if grouped_agents && dimension == Dimension::Agent {
        let key = agent_root_key("t");
        let join = agent_root_join("t");
        let distinct_turns = distinct_turn_expr("t");
        grouped_counts(
            conn,
            &format!(
                "SELECT {key}, {distinct_turns}
                 FROM telemetry_turns t {join}
                 WHERE t.ended_at >= ?1 AND t.ended_at < ?2
                 GROUP BY {key}"
            ),
            from,
            to,
        )?
    } else {
        let turn_key = match dimension {
            Dimension::Provider => "provider",
            Dimension::Agent => "session_id",
            Dimension::Model => "COALESCE(model, '')",
        };
        grouped_counts(
            conn,
            &format!(
                "SELECT {turn_key}, COUNT(DISTINCT COALESCE(turn_id, event_key))
                 FROM telemetry_turns
                 WHERE ended_at >= ?1 AND ended_at < ?2
                 GROUP BY {turn_key}"
            ),
            from,
            to,
        )?
    };

    // Edits carry no model of their own, so a model breakdown reaches it
    // through the turn the edit belongs to.
    //
    // Joined against one row per turn, not against the turn facts directly.
    // Codex writes several token_count records per `turn_id`, and while they
    // normally agree on the model, nothing enforces that. If two ever
    // disagreed, a plain join would emit the same edit under each model and
    // count one file twice — `COUNT(DISTINCT path)` only dedupes *within* a
    // group, not across them. Collapsing to a single model per turn first makes
    // the attribution exactly-once by construction rather than by assumption.
    let files_sql = match dimension {
        Dimension::Provider => "SELECT provider, COUNT(DISTINCT path) FROM telemetry_edits
             WHERE occurred_at >= ?1 AND occurred_at < ?2 GROUP BY provider"
            .to_string(),
        Dimension::Agent if grouped_agents => {
            let key = agent_root_key("e");
            let join = agent_root_join("e");
            format!(
                "SELECT {key}, COUNT(DISTINCT e.path) FROM telemetry_edits e {join}
                 WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2 GROUP BY {key}"
            )
        }
        Dimension::Agent => "SELECT session_id, COUNT(DISTINCT path) FROM telemetry_edits
             WHERE occurred_at >= ?1 AND occurred_at < ?2 GROUP BY session_id"
            .to_string(),
        Dimension::Model => "SELECT COALESCE(t.model, ''), COUNT(DISTINCT e.path)
             FROM telemetry_edits e
             LEFT JOIN (SELECT session_id, turn_id, MIN(model) AS model
                        FROM telemetry_turns WHERE turn_id IS NOT NULL
                        GROUP BY session_id, turn_id) t
               ON t.session_id = e.session_id AND t.turn_id = e.turn_id
             WHERE e.occurred_at >= ?1 AND e.occurred_at < ?2
             GROUP BY COALESCE(t.model, '')"
            .to_string(),
    };
    let files = grouped_counts(conn, &files_sql, from, to)?;

    Ok((turns, files))
}

fn grouped_counts(
    conn: &Connection,
    sql: &str,
    from: &str,
    to: &str,
) -> rusqlite::Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![from, to], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.collect()
}

/// One `COUNT(DISTINCT ...)` over a fact table for a window.
///
/// Exists because distinctness is the one property a rollup cannot preserve. A
/// pre-aggregated count answers "how many distinct values in this bucket", and
/// no combination of those answers "how many distinct values overall".
fn distinct_count(conn: &Connection, sql: &str, from: &str, to: &str) -> rusqlite::Result<i64> {
    conn.query_row(sql, params![from, to], |row| row.get(0))
}

/// Ranked rows for a dimension, most active first.
pub fn breakdown(
    conn: &Connection,
    dimension: Dimension,
    from: &str,
    to: &str,
    limit: usize,
) -> rusqlite::Result<Vec<BreakdownRow>> {
    breakdown_impl(conn, dimension, from, to, limit, false)
}

/// Ranked rows with verified provider-child facts grouped under their roster root.
pub fn grouped_breakdown(
    conn: &Connection,
    dimension: Dimension,
    from: &str,
    to: &str,
    limit: usize,
) -> rusqlite::Result<Vec<BreakdownRow>> {
    breakdown_impl(conn, dimension, from, to, limit, true)
}

fn breakdown_impl(
    conn: &Connection,
    dimension: Dimension,
    from: &str,
    to: &str,
    limit: usize,
    grouped_agents: bool,
) -> rusqlite::Result<Vec<BreakdownRow>> {
    let grouped_agent_rows = grouped_agents && dimension == Dimension::Agent;
    let column = if grouped_agent_rows {
        agent_root_key("r")
    } else {
        dimension.column().to_string()
    };
    // Distinct counts come from the facts; see `distinct_by_dimension`. Reading
    // them off the rollup here while `summary` reads them from the facts would
    // put two figures for the same quantity on one screen.
    let (turns_by_key, files_by_key) =
        distinct_by_dimension(conn, dimension, from, to, grouped_agent_rows)?;

    let (table, alias, join, session_count) = if grouped_agent_rows {
        (
            "telemetry_rollup_hourly",
            "r",
            agent_root_join("r"),
            format!("COUNT(DISTINCT {column})"),
        )
    } else {
        (
            "telemetry_rollup_hourly",
            "",
            String::new(),
            "COUNT(DISTINCT session_id)".to_string(),
        )
    };

    // Ranked by total active time, which is the one place a mixture is
    // unavoidable: rows measured differently still have to be put in some order.
    // Ordering is a weaker claim than reporting, and the split values travel
    // with every row so the UI never has to trust the ranking key.
    let sql = format!(
        "SELECT {column},
                COALESCE(SUM(measured_active_ms), 0),
                COALESCE(SUM(clustered_active_ms), 0),
                0,
                SUM(input_tokens),
                SUM(cached_input_tokens),
                SUM(cache_write_tokens),
                SUM(output_tokens),
                SUM(reasoning_tokens),
                0,
                COALESCE(SUM(lines_added), 0),
                COALESCE(SUM(lines_removed), 0),
                {session_count},
                MAX(tokens_reported)
         FROM {table} {alias} {join}
         WHERE bucket_start >= ?1 AND bucket_start < ?2
         GROUP BY {column}
         ORDER BY SUM(measured_active_ms) + SUM(clustered_active_ms) DESC, {column}
         LIMIT ?3"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![from, to, limit as i64], |row| {
        let key: String = row.get(0)?;
        Ok(BreakdownRow {
            active: ActiveTime {
                measured_ms: row.get(1)?,
                clustered_ms: row.get(2)?,
            },
            turns: turns_by_key.get(&key).copied().unwrap_or(0),
            tokens: TokenCounts {
                input_tokens: row.get(4)?,
                cached_input_tokens: row.get(5)?,
                cache_write_tokens: row.get(6)?,
                output_tokens: row.get(7)?,
                reasoning_tokens: row.get(8)?,
            },
            files_touched: files_by_key.get(&key).copied().unwrap_or(0),
            key,
            lines_added: row.get(10)?,
            lines_removed: row.get(11)?,
            agent_count: row.get(12)?,
            tokens_reported: row.get::<_, i64>(13)? != 0,
        })
    })?;
    rows.collect()
}

/// Per-bucket series for a dimension, for timelines and stacked charts.
pub fn series(
    conn: &Connection,
    dimension: Dimension,
    from: &str,
    to: &str,
) -> rusqlite::Result<Vec<SeriesPoint>> {
    series_impl(conn, dimension, from, to, false)
}

/// Per-bucket series with verified provider-child facts grouped under roots.
pub fn grouped_series(
    conn: &Connection,
    dimension: Dimension,
    from: &str,
    to: &str,
) -> rusqlite::Result<Vec<SeriesPoint>> {
    series_impl(conn, dimension, from, to, true)
}

fn series_impl(
    conn: &Connection,
    dimension: Dimension,
    from: &str,
    to: &str,
    grouped_agents: bool,
) -> rusqlite::Result<Vec<SeriesPoint>> {
    let grouped_agent_rows = grouped_agents && dimension == Dimension::Agent;
    let (from_alias, column_prefix, join, column) = if grouped_agent_rows {
        ("r", "r.", agent_root_join("r"), agent_root_key("r"))
    } else {
        ("", "", String::new(), dimension.column().to_string())
    };
    // New content processed: fresh input, cache writes, and output. Adding cache
    // *reads* here would dominate the series — they ran roughly ten times fresh
    // input on real sessions — and turn a token chart into a cache-hit chart.
    // Cache writes are the opposite case: content read for the first time, and
    // on claude they carry nearly all of it.
    //
    // The CASE mirrors `TokenCounts::processed_total` exactly: NULL when no
    // component was reported, and otherwise the sum with unreported parts
    // treated as zero. Writing it as a plain sum of COALESCEd columns would
    // instead report zero processed tokens for a provider that reported only
    // cache reads, which is a different claim from reporting none.
    let sql = format!(
        "SELECT {column_prefix}bucket_start, {column},
                COALESCE(SUM(measured_active_ms), 0),
                COALESCE(SUM(clustered_active_ms), 0),
                CASE WHEN SUM(input_tokens) IS NULL AND SUM(cache_write_tokens) IS NULL
                          AND SUM(output_tokens) IS NULL
                     THEN NULL
                     ELSE COALESCE(SUM(input_tokens), 0)
                          + COALESCE(SUM(cache_write_tokens), 0)
                          + COALESCE(SUM(output_tokens), 0)
                END
         FROM telemetry_rollup_hourly {from_alias} {join}
         WHERE {column_prefix}bucket_start >= ?1 AND {column_prefix}bucket_start < ?2
         GROUP BY {column_prefix}bucket_start, {column}
         ORDER BY {column_prefix}bucket_start, {column}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![from, to], |row| {
        Ok(SeriesPoint {
            bucket_start: row.get(0)?,
            key: row.get(1)?,
            active: ActiveTime {
                measured_ms: row.get(2)?,
                clustered_ms: row.get(3)?,
            },
            processed_tokens: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// Raw activity spans for the Gantt, carrying their method so clustered spans
/// stay visually distinct from measured ones.
pub fn activity_intervals(
    conn: &Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<Vec<IntervalFact>> {
    let mut stmt = conn.prepare(
        "SELECT session_id, provider, started_at, ended_at, last_event_at, event_count, method
         FROM telemetry_activity
         WHERE ended_at > ?1 AND started_at < ?2
         ORDER BY started_at",
    )?;
    let rows = stmt.query_map(params![from, to], |row| {
        Ok(IntervalFact {
            session_id: row.get(0)?,
            provider: row.get(1)?,
            started_at: row.get(2)?,
            ended_at: row.get(3)?,
            last_event_at: row.get(4)?,
            event_count: row.get(5)?,
            method: ActivityMethod::parse(&row.get::<_, String>(6)?)
                .unwrap_or(ActivityMethod::Clustered),
        })
    })?;
    rows.collect()
}

/// Latest limit observation per provider.
///
/// This is an account-level gauge, so the newest reading wins. Summing or
/// averaging across agents would report one account's usage several times over.
pub fn latest_limits(conn: &Connection) -> rusqlite::Result<Vec<LimitObservation>> {
    let mut stmt = conn.prepare(
        "SELECT provider, limit_id, observed_at, used_percent, window_minutes, resets_at, plan_type
         FROM telemetry_limits
         WHERE (provider, observed_at) IN (
             SELECT provider, MAX(observed_at) FROM telemetry_limits GROUP BY provider
         )
         ORDER BY provider",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LimitObservation {
            provider: row.get(0)?,
            limit_id: row.get(1)?,
            observed_at: row.get(2)?,
            used_percent: row.get(3)?,
            window_minutes: row.get(4)?,
            resets_at: row.get(5)?,
            plan_type: row.get(6)?,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::models::{Cursor, CursorKind, ParsedFacts, SourceKind, TurnFact};
    use crate::telemetry::rollup::recompute_buckets;
    use crate::telemetry::schema::run_telemetry_migrations;
    use crate::telemetry::store::{write_facts, SourceState};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_telemetry_migrations(&conn).unwrap();
        conn
    }

    fn state(provider: &str, session: &str) -> SourceState {
        let path = format!("{provider}-{session}");
        SourceState {
            source_key: crate::telemetry::store::source_key(provider, session, &path),
            source_path: path,
            session_id: session.into(),
            provider_session_id: None,
            provider: provider.into(),
            source_kind: SourceKind::Jsonl,
            cursor: Cursor::new(CursorKind::ByteOffset, 0),
            last_size: 0,
            last_modified: None,
            parser_version: 1,
            fingerprint: None,
            carry: Default::default(),
        }
    }

    fn turn(session: &str, provider: &str, ended_at: &str, input: Option<i64>) -> TurnFact {
        TurnFact {
            event_key: format!("{session}-{provider}-{ended_at}"),
            session_id: session.into(),
            provider: provider.into(),
            turn_id: Some(format!("{session}-{ended_at}")),
            model: Some("m1".into()),
            effort: None,
            started_at: None,
            ended_at: ended_at.into(),
            input_tokens: input,
            cached_input_tokens: input.map(|_| 500),
            // Also conditional: a provider with no token accounting reports
            // nothing, not a zero in one field and nulls in the rest.
            cache_write_tokens: input.map(|_| 0),
            output_tokens: input.map(|_| 20),
            reasoning_tokens: input.map(|_| 5),
            context_window: None,
            cost_usd: None,
        }
    }

    fn ingest(
        conn: &mut Connection,
        provider: &str,
        session: &str,
        facts: ParsedFacts,
        intervals: Vec<IntervalFact>,
    ) {
        let st = state(provider, session);
        let dirty = write_facts(conn, &facts, &intervals, &st).unwrap();
        recompute_buckets(conn, &dirty).unwrap();
    }

    fn seed(conn: &mut Connection) {
        ingest(
            conn,
            "codex",
            "agent-1",
            ParsedFacts {
                turns: vec![turn(
                    "agent-1",
                    "codex",
                    "2026-08-13T18:10:00.000Z",
                    Some(100),
                )],
                ..Default::default()
            },
            vec![IntervalFact {
                session_id: "agent-1".into(),
                provider: "codex".into(),
                started_at: "2026-08-13T18:00:00.000Z".into(),
                ended_at: "2026-08-13T18:30:00.000Z".into(),
                last_event_at: "2026-08-13T18:30:00.000Z".into(),
                event_count: 9,
                method: ActivityMethod::Clustered,
            }],
        );
        ingest(
            conn,
            "opencode",
            "agent-2",
            ParsedFacts {
                turns: vec![turn(
                    "agent-2",
                    "opencode",
                    "2026-08-13T18:20:00.000Z",
                    Some(40),
                )],
                ..Default::default()
            },
            vec![IntervalFact {
                session_id: "agent-2".into(),
                provider: "opencode".into(),
                started_at: "2026-08-13T18:05:00.000Z".into(),
                ended_at: "2026-08-13T18:15:00.000Z".into(),
                last_event_at: "2026-08-13T18:15:00.000Z".into(),
                event_count: 1,
                method: ActivityMethod::Measured,
            }],
        );
    }

    const FROM: &str = "2026-08-13T00:00:00.000Z";
    const TO: &str = "2026-08-14T00:00:00.000Z";

    #[test]
    fn dimension_round_trips_and_rejects_anything_else() {
        for dimension in [Dimension::Provider, Dimension::Agent, Dimension::Model] {
            assert_eq!(Dimension::parse(dimension.as_str()), Some(dimension));
        }
        // The store's own spelling is accepted too, since callers see both.
        assert_eq!(Dimension::parse("session_id"), Some(Dimension::Agent));

        // `column()` is interpolated into SQL, so an unrecognized name has to
        // fail rather than fall through to a default. A default would let a
        // caller-supplied string decide what gets grouped on, and this is the
        // only gate between the two.
        assert_eq!(
            Dimension::parse("provider; DROP TABLE telemetry_turns"),
            None
        );
        assert_eq!(Dimension::parse(""), None);
        assert_eq!(Dimension::parse("PROVIDER"), None);
    }

    #[test]
    fn summary_aggregates_across_agents() {
        let mut conn = db();
        seed(&mut conn);
        let summary = summary(&conn, FROM, TO).unwrap();
        assert_eq!(summary.turns, 2);
        assert_eq!(summary.tokens.input_tokens, Some(140));
        assert_eq!(summary.agent_count, 2);
    }

    #[test]
    fn summary_keeps_measured_and_clustered_separate() {
        let mut conn = db();
        seed(&mut conn);
        let summary = summary(&conn, FROM, TO).unwrap();
        assert_eq!(summary.active.measured_ms, 10 * 60 * 1000);
        assert_eq!(summary.active.clustered_ms, 30 * 60 * 1000);
        // Both methods contributed, so any single figure shown is a mixture and
        // the caller is told as much rather than handed a pre-blended total.
        assert!(summary.active.is_mixed());
    }

    #[test]
    fn summary_of_an_empty_horizon_is_zeroes_not_an_error() {
        let conn = db();
        let summary = summary(&conn, FROM, TO).unwrap();
        assert_eq!(summary.turns, 0);
        assert_eq!(summary.agent_count, 0);
        assert_eq!(summary.active.measured_ms, 0);
        // No turns at all is not a report of zero tokens.
        assert_eq!(summary.tokens.input_tokens, None);
        assert!(!summary.tokens.any_reported());
    }

    #[test]
    fn horizon_bounds_exclude_the_upper_edge() {
        let mut conn = db();
        seed(&mut conn);
        let empty = summary(
            &conn,
            "2026-08-14T00:00:00.000Z",
            "2026-08-15T00:00:00.000Z",
        )
        .unwrap();
        assert_eq!(empty.turns, 0);
    }

    #[test]
    fn breakdown_by_provider_ranks_by_active_time() {
        let mut conn = db();
        seed(&mut conn);
        let rows = breakdown(&conn, Dimension::Provider, FROM, TO, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "codex");
        assert_eq!(rows[0].active.clustered_ms, 30 * 60 * 1000);
        assert_eq!(rows[0].active.measured_ms, 0);
        assert_eq!(rows[1].key, "opencode");
    }

    #[test]
    fn breakdown_keeps_cache_reads_out_of_the_processed_total() {
        let mut conn = db();
        seed(&mut conn);
        let rows = breakdown(&conn, Dimension::Provider, FROM, TO, 10).unwrap();
        let codex = rows.iter().find(|row| row.key == "codex").unwrap();
        assert_eq!(codex.tokens.input_tokens, Some(100));
        assert_eq!(codex.tokens.cached_input_tokens, Some(500));
        // Cache reads stay addressable but out of the headline figure; folding
        // them in would make this row read six times busier than it was.
        assert_eq!(codex.tokens.processed_total(), Some(120));
    }

    #[test]
    fn breakdown_flags_a_provider_with_no_token_accounting() {
        let mut conn = db();
        ingest(
            &mut conn,
            "antigravity",
            "agent-3",
            ParsedFacts {
                turns: vec![turn(
                    "agent-3",
                    "antigravity",
                    "2026-08-13T18:10:00.000Z",
                    None,
                )],
                ..Default::default()
            },
            vec![],
        );
        let rows = breakdown(&conn, Dimension::Provider, FROM, TO, 10).unwrap();
        let row = rows.iter().find(|row| row.key == "antigravity").unwrap();
        assert!(!row.tokens_reported);
        // Null rather than zero, so it cannot be ranked as the cheapest
        // provider by anything that sorts on token counts.
        assert_eq!(row.tokens.input_tokens, None);
        assert_eq!(row.tokens.processed_total(), None);
    }

    #[test]
    fn breakdown_by_agent_separates_sessions() {
        let mut conn = db();
        seed(&mut conn);
        let rows = breakdown(&conn, Dimension::Agent, FROM, TO, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.agent_count == 1));
    }

    #[test]
    fn breakdown_respects_the_limit() {
        let mut conn = db();
        seed(&mut conn);
        assert_eq!(
            breakdown(&conn, Dimension::Provider, FROM, TO, 1)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn series_returns_one_row_per_bucket_and_group() {
        let mut conn = db();
        seed(&mut conn);
        let rows = series(&conn, Dimension::Provider, FROM, TO).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|row| row.bucket_start == "2026-08-13T18:00:00.000Z"));

        let codex = rows.iter().find(|row| row.key == "codex").unwrap();
        assert_eq!(codex.active.clustered_ms, 30 * 60 * 1000);
        assert_eq!(codex.processed_tokens, Some(120));
    }

    #[test]
    fn series_reports_no_tokens_rather_than_zero_for_an_unreporting_provider() {
        let mut conn = db();
        ingest(
            &mut conn,
            "antigravity",
            "agent-3",
            ParsedFacts {
                turns: vec![turn(
                    "agent-3",
                    "antigravity",
                    "2026-08-13T18:10:00.000Z",
                    None,
                )],
                ..Default::default()
            },
            vec![],
        );
        let rows = series(&conn, Dimension::Provider, FROM, TO).unwrap();
        let row = rows.iter().find(|row| row.key == "antigravity").unwrap();
        assert_eq!(row.processed_tokens, None);
    }

    #[test]
    fn series_and_breakdown_agree_on_what_processed_means() {
        // Two code paths compute the same quantity, one in SQL and one in Rust.
        // They have to answer identically, including for the awkward case of a
        // provider that reported cache reads and nothing else — zero processed
        // tokens is a different claim from no token accounting.
        let mut conn = db();
        let cache_only = TurnFact {
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: Some(4_096),
            cache_write_tokens: None,
            reasoning_tokens: None,
            ..turn("agent-9", "codex", "2026-08-13T18:10:00.000Z", None)
        };
        ingest(
            &mut conn,
            "codex",
            "agent-9",
            ParsedFacts {
                turns: vec![cache_only],
                ..Default::default()
            },
            vec![],
        );

        let point = series(&conn, Dimension::Agent, FROM, TO)
            .unwrap()
            .into_iter()
            .find(|row| row.key == "agent-9")
            .unwrap();
        let row = breakdown(&conn, Dimension::Agent, FROM, TO, 10)
            .unwrap()
            .into_iter()
            .find(|row| row.key == "agent-9")
            .unwrap();

        assert_eq!(row.tokens.cached_input_tokens, Some(4_096));
        assert!(row.tokens_reported);
        assert_eq!(row.tokens.processed_total(), None);
        assert_eq!(point.processed_tokens, row.tokens.processed_total());
    }

    #[test]
    fn intervals_carry_their_method_for_the_gantt() {
        let mut conn = db();
        seed(&mut conn);
        let intervals = activity_intervals(&conn, FROM, TO).unwrap();
        assert_eq!(intervals.len(), 2);
        assert!(intervals
            .iter()
            .any(|interval| interval.method == ActivityMethod::Measured));
        assert!(intervals
            .iter()
            .any(|interval| interval.method == ActivityMethod::Clustered));
    }

    #[test]
    fn intervals_overlapping_the_window_edge_are_included() {
        let mut conn = db();
        seed(&mut conn);
        // Window starts mid-interval; the span still belongs to the view.
        let intervals = activity_intervals(
            &conn,
            "2026-08-13T18:20:00.000Z",
            "2026-08-13T18:25:00.000Z",
        )
        .unwrap();
        assert_eq!(intervals.len(), 1);
        assert_eq!(intervals[0].session_id, "agent-1");
    }

    #[test]
    fn latest_limit_per_provider_wins() {
        let conn = db();
        for (observed, percent) in [
            ("2026-08-13T18:00:00.000Z", 20.0),
            ("2026-08-13T19:00:00.000Z", 43.0),
        ] {
            conn.execute(
                "INSERT INTO telemetry_limits (provider, limit_id, observed_at, used_percent, plan_type)
                 VALUES ('codex', 'codex', ?1, ?2, 'prolite')",
                params![observed, percent],
            )
            .unwrap();
        }
        let limits = latest_limits(&conn).unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].used_percent, Some(43.0));
    }

    #[test]
    fn limits_are_not_summed_across_observations() {
        let conn = db();
        for session in ["a", "b"] {
            conn.execute(
                "INSERT INTO telemetry_limits (provider, limit_id, observed_at, used_percent)
                 VALUES ('codex', ?1, '2026-08-13T18:00:00.000Z', 43.0)",
                params![session],
            )
            .unwrap();
        }
        // Two agents on one account observing 43% are observing the same 43%.
        let limits = latest_limits(&conn).unwrap();
        assert!(limits.iter().all(|limit| limit.used_percent == Some(43.0)));
    }
}
