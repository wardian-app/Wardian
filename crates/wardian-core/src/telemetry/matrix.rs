//! The read shape the Dashboard is built on: rows × time × one measure.
//!
//! Two dimensions are the point. A single one-dimensional list cannot answer
//! "how much did this agent edit last Tuesday", and a stack of such lists makes
//! the reader join them by hand. Rows are a dimension, columns are time, and the
//! measure is the thing you change.
//!
//! **Resolution comes from the window, not from the rollup.** The rollup's
//! hourly grain is a storage decision made for horizon summaries; letting it set
//! the display resolution was a real defect — a four-hour window rendered as
//! five enormous blocks and lost every distinction inside them. Cells therefore
//! read the **fact tables**, which carry exact timestamps, and the grain is
//! chosen to fill the axis with a legible number of columns down to five
//! minutes. The fact tables are indexed on the time columns these scan, and the
//! rollup remains the fast path for [`crate::telemetry::query::summary`].
//!
//! Buckets are **dense**. Every row carries a cell for every bucket, including
//! the empty ones, because the columns are a time axis: dropping quiet buckets
//! would compress it and make two rows with different gaps appear to line up.

use crate::telemetry::attribution::{agent_root_join, agent_root_key, distinct_turn_expr};
use crate::telemetry::horizon::HorizonWindow;
use crate::telemetry::query::Dimension;
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What a cell counts.
///
/// Cost is deliberately absent: only opencode reports it, so a column blank for
/// most of the habitat would invite comparing agents on a figure most of them
/// cannot produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// Wall-clock time the agent was working, from the real activity spans.
    ///
    /// Measured where a provider reports durations, inferred from event gaps
    /// where it does not; the two are summed because the distinction is
    /// accurate but not actionable.
    ActiveMs,
    Turns,
    /// New prompt tokens — cache reads excluded. See `TurnFact::input_tokens`.
    FreshTokens,
    /// Prompt tokens served from cache. Its own measure because it dwarfs fresh
    /// input, running roughly 50x on a real habitat.
    CachedTokens,
    /// Prompt tokens written *into* the cache — content read for the first time
    /// and billed at or above the fresh-input rate. Distinct from
    /// [`Measure::CachedTokens`] in every way that matters: that is replayed
    /// work, this is new work. Claude routes nearly all of its fresh prompt
    /// content through here, so its absence understated real sessions ~5x.
    CacheWriteTokens,
    OutputTokens,
    ReasoningTokens,
    /// Fresh input, cache writes, and output: new content processed.
    TotalTokens,
    /// Share of the prompt served from cache, as a percentage.
    ///
    /// A ratio, not a count. It cannot be summed across buckets or rows — see
    /// [`Measure::is_ratio`].
    CacheHitRate,
    Files,
    LinesAdded,
    LinesRemoved,
    LinesChanged,
}

impl Measure {
    pub fn as_str(self) -> &'static str {
        match self {
            Measure::ActiveMs => "active_ms",
            Measure::Turns => "turns",
            Measure::FreshTokens => "fresh_tokens",
            Measure::CachedTokens => "cached_tokens",
            Measure::CacheWriteTokens => "cache_write_tokens",
            Measure::OutputTokens => "output_tokens",
            Measure::ReasoningTokens => "reasoning_tokens",
            Measure::TotalTokens => "total_tokens",
            Measure::CacheHitRate => "cache_hit_rate",
            Measure::Files => "files",
            Measure::LinesAdded => "lines_added",
            Measure::LinesRemoved => "lines_removed",
            Measure::LinesChanged => "lines_changed",
        }
    }

    /// Resolve a caller-supplied name. Closed and total, like
    /// [`Dimension::parse`], because the result selects a SQL expression.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "active_ms" => Measure::ActiveMs,
            "turns" => Measure::Turns,
            "fresh_tokens" => Measure::FreshTokens,
            "cached_tokens" => Measure::CachedTokens,
            "cache_write_tokens" => Measure::CacheWriteTokens,
            "output_tokens" => Measure::OutputTokens,
            "reasoning_tokens" => Measure::ReasoningTokens,
            "total_tokens" => Measure::TotalTokens,
            "cache_hit_rate" => Measure::CacheHitRate,
            "files" => Measure::Files,
            "lines_added" => Measure::LinesAdded,
            "lines_removed" => Measure::LinesRemoved,
            "lines_changed" => Measure::LinesChanged,
            _ => return None,
        })
    }

    /// Whether this measure is a distinct count, and therefore cannot be summed
    /// across buckets or across rows.
    pub fn is_distinct_count(self) -> bool {
        matches!(self, Measure::Turns | Measure::Files)
    }

    /// Whether this measure is a ratio, and therefore unsummable.
    ///
    /// Separate from [`Self::is_distinct_count`] because the two are unsummable
    /// for different reasons. A distinct count would double count a repeated
    /// member; a ratio has no meaningful sum at all, and averaging its cells
    /// weights a quiet bucket the same as a busy one. Both must be recomputed
    /// for a total rather than accumulated, which the SQL expression does by
    /// deriving the ratio from the components inside whatever group it is given.
    pub fn is_ratio(self) -> bool {
        matches!(self, Measure::CacheHitRate)
    }

    /// Which fact table this measure is drawn from.
    fn source(self) -> MeasureSource {
        match self {
            Measure::ActiveMs => MeasureSource::Activity,
            Measure::Turns
            | Measure::FreshTokens
            | Measure::CachedTokens
            | Measure::CacheWriteTokens
            | Measure::OutputTokens
            | Measure::ReasoningTokens
            | Measure::TotalTokens
            | Measure::CacheHitRate => MeasureSource::Turns,
            Measure::Files
            | Measure::LinesAdded
            | Measure::LinesRemoved
            | Measure::LinesChanged => MeasureSource::Edits,
        }
    }

    /// The aggregate over the fact table's own columns.
    fn fact_expr(self) -> &'static str {
        match self {
            Measure::Turns => "COUNT(DISTINCT COALESCE(turn_id, event_key))",
            Measure::FreshTokens => "COALESCE(SUM(input_tokens), 0)",
            Measure::CachedTokens => "COALESCE(SUM(cached_input_tokens), 0)",
            Measure::CacheWriteTokens => "COALESCE(SUM(cache_write_tokens), 0)",
            Measure::OutputTokens => "COALESCE(SUM(output_tokens), 0)",
            Measure::ReasoningTokens => "COALESCE(SUM(reasoning_tokens), 0)",
            Measure::TotalTokens => {
                "COALESCE(SUM(input_tokens), 0) + COALESCE(SUM(cache_write_tokens), 0)                  + COALESCE(SUM(output_tokens), 0)"
            }
            // Recomputed from components rather than averaged, which is what
            // makes a row total meaningful. Scaled to a percentage; the zero
            // guard keeps a bucket with no prompt tokens at 0 instead of NULL.
            Measure::CacheHitRate => {
                "CASE WHEN COALESCE(SUM(input_tokens), 0)                           + COALESCE(SUM(cache_write_tokens), 0)                           + COALESCE(SUM(cached_input_tokens), 0) = 0                       THEN 0                       ELSE 100 * COALESCE(SUM(cached_input_tokens), 0)                            / (COALESCE(SUM(input_tokens), 0)                               + COALESCE(SUM(cache_write_tokens), 0)                               + COALESCE(SUM(cached_input_tokens), 0)) END"
            }
            Measure::Files => "COUNT(DISTINCT path)",
            Measure::LinesAdded => "COALESCE(SUM(lines_added), 0)",
            Measure::LinesRemoved => "COALESCE(SUM(lines_removed), 0)",
            Measure::LinesChanged => "COALESCE(SUM(lines_added), 0) + COALESCE(SUM(lines_removed), 0)",
            // Never reaches SQL; activity is distributed across buckets in Rust
            // because a span has to be split, not grouped.
            Measure::ActiveMs => "0",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasureSource {
    Turns,
    Edits,
    Activity,
}

impl MeasureSource {
    fn table(self) -> &'static str {
        match self {
            MeasureSource::Turns => "telemetry_turns",
            MeasureSource::Edits => "telemetry_edits",
            MeasureSource::Activity => "telemetry_activity",
        }
    }

    /// The column a row is placed in time by.
    fn time_column(self) -> &'static str {
        match self {
            MeasureSource::Turns => "ended_at",
            MeasureSource::Edits => "occurred_at",
            MeasureSource::Activity => "started_at",
        }
    }
}

/// How wide one column is.
///
/// Sub-hour grains exist because the previous version bottomed out at an hour
/// and a short window became a handful of blocks with no detail inside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grain {
    Minute5,
    Minute15,
    Hour,
    SixHour,
    Day,
}

impl Grain {
    pub fn as_str(self) -> &'static str {
        match self {
            Grain::Minute5 => "minute5",
            Grain::Minute15 => "minute15",
            Grain::Hour => "hour",
            Grain::SixHour => "six_hour",
            Grain::Day => "day",
        }
    }

    /// Length in seconds. `None` for days, which are not a fixed duration once
    /// a daylight-saving change is in the window.
    fn seconds(self) -> Option<i64> {
        Some(match self {
            Grain::Minute5 => 300,
            Grain::Minute15 => 900,
            Grain::Hour => 3_600,
            Grain::SixHour => 21_600,
            Grain::Day => return None,
        })
    }

    /// The finest grain that keeps the axis to a legible number of columns.
    ///
    /// Aimed at roughly a hundred: enough that an hour of work is several cells
    /// wide rather than one block, few enough to render and label.
    pub fn for_window(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        let span = to - from;
        if span <= Duration::hours(8) {
            Grain::Minute5
        } else if span <= Duration::days(1) {
            Grain::Minute15
        } else if span <= Duration::days(5) {
            Grain::Hour
        } else if span <= Duration::days(31) {
            Grain::SixHour
        } else {
            Grain::Day
        }
    }

    /// Raise this grain to at least `floor`.
    fn at_least(self, floor: Grain) -> Self {
        if self < floor {
            floor
        } else {
            self
        }
    }

    /// How many buckets this grain lays over a window.
    ///
    /// Answered by laying the buckets out rather than by dividing the span. The
    /// axis starts at the grain boundary *below* `from`, so a window that begins
    /// mid-bucket covers one more column than its length divided by the bucket
    /// width — and a divided count silently under-reports it, letting a capped
    /// axis come back wider than the cap allowed. Days compound this, being not
    /// a fixed number of seconds across a daylight-saving change.
    ///
    /// Deriving it from [`bucket_bounds`] is what stops the two rules drifting;
    /// it is also therefore capped at `MAX_BUCKETS`, which is far above any cap
    /// a caller asks for.
    pub fn bucket_count(self, from: DateTime<Utc>, to: DateTime<Utc>) -> usize {
        bucket_bounds(from, to, self).len()
    }

    /// The finest grain whose axis fits within `max_buckets` columns.
    ///
    /// A sparkline is a shape read at a glance in a table cell, not an axis to
    /// look values up on, so it wants tens of buckets where the Analytics matrix
    /// wants hundreds. Coarsening by choosing a grain keeps every cell a real
    /// aggregate over a real interval; folding fine buckets together afterwards
    /// would silently add distinct counts that cannot be added.
    pub fn for_window_within(from: DateTime<Utc>, to: DateTime<Utc>, max_buckets: usize) -> Self {
        let natural = Self::for_window(from, to);
        [
            Grain::Minute5,
            Grain::Minute15,
            Grain::Hour,
            Grain::SixHour,
            Grain::Day,
        ]
        .into_iter()
        .filter(|candidate| *candidate >= natural)
        .find(|candidate| candidate.bucket_count(from, to) <= max_buckets)
        .unwrap_or(Grain::Day)
    }
}

/// One row of the matrix: a dimension value and its cells, in bucket order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixRow {
    pub key: String,
    /// One value per bucket in [`Matrix::buckets`], same length, same order.
    pub cells: Vec<i64>,
    /// The row's value over the whole window.
    ///
    /// For a distinct count this is **not** the sum of `cells`: a turn spanning
    /// two buckets is one turn in the total and appears in both cells. Both are
    /// correct answers to different questions, which is why the total is
    /// computed separately rather than added up.
    pub total: i64,
}

impl MatrixRow {
    /// An all-zero row, so a caller can show a roster entry that did nothing in
    /// this window rather than dropping it.
    pub fn empty(key: String, buckets: usize) -> Self {
        Self {
            key,
            cells: vec![0; buckets],
            total: 0,
        }
    }
}

/// A dense rows × buckets grid for one measure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    pub dimension: Dimension,
    pub measure: Measure,
    pub grain: Grain,
    /// Bucket start instants, ascending, with no gaps.
    pub buckets: Vec<String>,
    /// Rows, largest total first.
    pub rows: Vec<MatrixRow>,
    /// Largest single cell, so a heatmap can normalize without a second pass.
    /// Zero when the window is empty.
    pub max_cell: i64,
    /// True when `cells` do not sum to `total`, i.e. the measure is a distinct
    /// count. Lets a surface avoid presenting a row as if it added up.
    pub cells_are_not_additive: bool,
}

/// Build the matrix for a window at the grain the window implies.
pub fn matrix(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measure: Measure,
    row_limit: usize,
) -> rusqlite::Result<Matrix> {
    matrix_at(conn, window, dimension, measure, row_limit, None)
}

/// Build the matrix, optionally capping how many buckets the axis may have.
///
/// `max_buckets` can only coarsen. A caller asking for at most fifty buckets
/// over a year still gets days rather than something finer, because the natural
/// grain already fits and refining it would produce an axis nobody asked for.
pub fn matrix_at(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measure: Measure,
    row_limit: usize,
    max_buckets: Option<usize>,
) -> rusqlite::Result<Matrix> {
    matrix_at_impl(
        conn,
        window,
        dimension,
        measure,
        row_limit,
        max_buckets,
        false,
    )
}

/// Build a matrix after grouping verified provider-child facts under roster roots.
pub fn grouped_matrix_at(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measure: Measure,
    row_limit: usize,
    max_buckets: Option<usize>,
) -> rusqlite::Result<Matrix> {
    matrix_at_impl(
        conn,
        window,
        dimension,
        measure,
        row_limit,
        max_buckets,
        true,
    )
}

fn matrix_at_impl(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measure: Measure,
    row_limit: usize,
    max_buckets: Option<usize>,
    grouped_agents: bool,
) -> rusqlite::Result<Matrix> {
    let grouped_agent_rows = grouped_agents && dimension == Dimension::Agent;
    let from = parse_instant(&window.from);
    let to = parse_instant(&window.to);

    // Activity facts carry a session and a provider but no model, so a model
    // view of active time cannot come from them. Rather than fabricate an
    // attribution, that one combination falls back to the rollup, whose grain
    // is an hour.
    let needs_rollup = measure == Measure::ActiveMs && dimension == Dimension::Model;
    let mut grain = if needs_rollup {
        Grain::for_window(from, to).at_least(Grain::Hour)
    } else {
        Grain::for_window(from, to)
    };
    if let Some(cap) = max_buckets {
        grain = grain.at_least(Grain::for_window_within(from, to, cap));
    }

    let bounds = bucket_bounds(from, to, grain);
    let buckets: Vec<String> = bounds
        .iter()
        .map(|(start, _)| format_instant(*start))
        .collect();

    let cells = if needs_rollup {
        rollup_active_cells(conn, window, grain)?
    } else if measure == Measure::ActiveMs {
        activity_cells(conn, window, dimension, &bounds, grouped_agent_rows)?
    } else {
        fact_cells(conn, window, dimension, measure, grain, grouped_agent_rows)?
    };
    let totals = row_totals(
        conn,
        window,
        dimension,
        measure,
        needs_rollup,
        grouped_agent_rows,
    )?;

    let index: HashMap<&str, usize> = buckets
        .iter()
        .enumerate()
        .map(|(position, label)| (label.as_str(), position))
        .collect();

    let mut rows: Vec<MatrixRow> = totals
        .into_iter()
        .map(|(key, total)| {
            let mut cells_for_row = vec![0_i64; buckets.len()];
            if let Some(values) = cells.get(&key) {
                for (bucket, value) in values {
                    // A bucket outside the generated axis can only come from a
                    // row on the window edge; dropping it is right, because the
                    // axis defines the window being displayed.
                    if let Some(position) = index.get(bucket.as_str()) {
                        cells_for_row[*position] = *value;
                    }
                }
            }
            MatrixRow {
                key,
                cells: cells_for_row,
                total,
            }
        })
        .collect();

    rows.sort_by(|left, right| right.total.cmp(&left.total).then(left.key.cmp(&right.key)));
    rows.truncate(row_limit);

    let max_cell = rows
        .iter()
        .flat_map(|row| row.cells.iter().copied())
        .max()
        .unwrap_or(0);

    Ok(Matrix {
        dimension,
        measure,
        grain,
        buckets,
        rows,
        max_cell,
        // A ratio is unsummable for a different reason than a distinct count,
        // but the surface consequence is the same: its cells do not add up to
        // its total, and nothing may present them as if they do.
        cells_are_not_additive: measure.is_distinct_count() || measure.is_ratio(),
    })
}

fn parse_instant(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap())
}

fn format_instant(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Guards a pathological window from generating an unbounded axis.
const MAX_BUCKETS: usize = 400;

/// Half-open `[start, end)` bounds for every bucket in the window.
///
/// Returned as instants rather than only labels because active time has to be
/// *split* across buckets by overlap, which needs the boundaries themselves.
///
/// Day buckets are **local** days so the columns line up with what the viewer
/// calls a day; a UTC boundary would put an evening's work in tomorrow's column
/// for anyone west of Greenwich.
fn bucket_bounds(
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    grain: Grain,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut bounds = Vec::new();
    match grain.seconds() {
        Some(step) => {
            let mut cursor = floor_to_seconds(from, step);
            while cursor < to && bounds.len() < MAX_BUCKETS {
                let end = cursor + Duration::seconds(step);
                bounds.push((cursor, end));
                cursor = end;
            }
        }
        None => {
            let mut cursor = local_midnight(from);
            while cursor < to && bounds.len() < MAX_BUCKETS {
                // Stepping the local date and re-resolving, rather than adding
                // 24 hours, so a daylight-saving change still yields one column
                // per calendar day.
                let end = local_midnight(cursor + Duration::hours(36));
                bounds.push((cursor, end));
                cursor = end;
            }
        }
    }
    bounds
}

fn floor_to_seconds(instant: DateTime<Utc>, step: i64) -> DateTime<Utc> {
    let seconds = instant.timestamp();
    let floored = seconds - seconds.rem_euclid(step);
    Utc.timestamp_opt(floored, 0).single().unwrap_or(instant)
}

fn local_midnight(instant: DateTime<Utc>) -> DateTime<Utc> {
    let local = instant.with_timezone(&Local);
    Local
        .with_ymd_and_hms(local.year(), local.month(), local.day(), 0, 0, 0)
        .single()
        .map(|midnight| midnight.with_timezone(&Utc))
        .unwrap_or(instant)
}

/// SQL mapping a timestamp column onto a bucket label matching
/// [`bucket_bounds`].
fn bucket_expr(column: &str, grain: Grain) -> String {
    match grain.seconds() {
        // Floor through epoch seconds, which works for any fixed-length grain
        // and keeps the label byte-identical to the generated axis.
        Some(step) => format!(
            "strftime('%Y-%m-%dT%H:%M:%S.000Z', \
             (CAST(strftime('%s', {column}) AS INTEGER) / {step}) * {step}, 'unixepoch')"
        ),
        None => format!(
            "strftime('%Y-%m-%dT%H:%M:%S.000Z', datetime(date({column}, 'localtime'), 'utc'))"
        ),
    }
}

type CellMap = HashMap<String, Vec<(String, i64)>>;

/// The dimension's key expression against a fact table.
fn fact_key(dimension: Dimension, source: MeasureSource) -> &'static str {
    match (dimension, source) {
        (Dimension::Provider, _) => "provider",
        (Dimension::Agent, _) => "session_id",
        (Dimension::Model, MeasureSource::Turns) => "COALESCE(model, '')",
        // Edits reach a model through their turn; see `edit_model_join`.
        (Dimension::Model, MeasureSource::Edits) => "COALESCE(t.model, '')",
        (Dimension::Model, MeasureSource::Activity) => "''",
    }
}

/// Joins one model per turn onto edits.
///
/// Codex writes several token records per `turn_id`, and while they normally
/// agree on the model nothing enforces it. Joining the turn facts directly
/// would emit one edit under each model and count a single file twice, because
/// `COUNT(DISTINCT path)` dedupes within a group and not across them.
const EDIT_MODEL_JOIN: &str = "LEFT JOIN (SELECT session_id, turn_id, MIN(model) AS model
                                          FROM telemetry_turns WHERE turn_id IS NOT NULL
                                          GROUP BY session_id, turn_id) t
                                 ON t.session_id = e.session_id AND t.turn_id = e.turn_id";

fn fact_cells(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measure: Measure,
    grain: Grain,
    grouped_agents: bool,
) -> rusqlite::Result<CellMap> {
    let source = measure.source();
    let table = source.table();
    let time = source.time_column();
    let needs_join = dimension == Dimension::Model && source == MeasureSource::Edits;
    // Aliased whenever the model join is in play, so `e.path` resolves.
    let (alias, join, column) = if needs_join {
        ("e", EDIT_MODEL_JOIN.to_string(), format!("e.{time}"))
    } else if grouped_agents {
        ("f", agent_root_join("f"), format!("f.{time}"))
    } else {
        ("", String::new(), time.to_string())
    };
    let expr = if grouped_agents && measure == Measure::Turns {
        distinct_turn_expr(alias)
    } else if needs_join {
        measure.fact_expr().replace("path", "e.path")
    } else {
        measure.fact_expr().to_string()
    };
    let key = if grouped_agents {
        agent_root_key(alias)
    } else {
        fact_key(dimension, source).to_string()
    };
    let bucket = bucket_expr(&column, grain);

    let sql = format!(
        "SELECT {key}, {bucket} AS bucket, {expr}
         FROM {table} {alias} {join}
         WHERE {column} >= ?1 AND {column} < ?2
         GROUP BY {key}, bucket"
    );
    collect_cells(conn, &sql, window)
}

/// Active milliseconds per bucket, from the real spans.
///
/// Distributed in Rust rather than grouped in SQL because an interval has to be
/// **split** across the buckets it crosses. Grouping it by its start would put a
/// two-hour span entirely in its first bucket, which is exactly the loss of
/// resolution this view exists to avoid.
fn activity_cells(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    bounds: &[(DateTime<Utc>, DateTime<Utc>)],
    grouped_agents: bool,
) -> rusqlite::Result<CellMap> {
    let (key, join) = if grouped_agents {
        (agent_root_key("a"), agent_root_join("a"))
    } else {
        (
            match dimension {
                Dimension::Provider => "provider".to_string(),
                // Model is handled by the rollup path; activity carries no model.
                _ => "session_id".to_string(),
            },
            String::new(),
        )
    };
    let sql = format!(
        "SELECT {key}, a.started_at, a.ended_at FROM telemetry_activity a {join}
         WHERE ended_at > ?1 AND started_at < ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let spans = stmt.query_map(params![window.from, window.to], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut totals: HashMap<String, Vec<i64>> = HashMap::new();
    for span in spans {
        let (key, started, ended) = span?;
        let start = parse_instant(&started);
        let end = parse_instant(&ended);
        let row = totals
            .entry(key)
            .or_insert_with(|| vec![0_i64; bounds.len()]);
        for (position, (bucket_start, bucket_end)) in bounds.iter().enumerate() {
            let overlap_start = start.max(*bucket_start);
            let overlap_end = end.min(*bucket_end);
            let overlap = (overlap_end - overlap_start).num_milliseconds();
            if overlap > 0 {
                row[position] += overlap;
            }
        }
    }

    Ok(totals
        .into_iter()
        .map(|(key, values)| {
            let cells = values
                .into_iter()
                .enumerate()
                .filter(|(_, value)| *value > 0)
                .map(|(position, value)| (format_instant(bounds[position].0), value))
                .collect();
            (key, cells)
        })
        .collect())
}

/// Active milliseconds by model, which only the rollup can attribute.
fn rollup_active_cells(
    conn: &Connection,
    window: &HorizonWindow,
    grain: Grain,
) -> rusqlite::Result<CellMap> {
    let bucket = bucket_expr("bucket_start", grain);
    let sql = format!(
        "SELECT model, {bucket} AS bucket,
                COALESCE(SUM(measured_active_ms), 0) + COALESCE(SUM(clustered_active_ms), 0)
         FROM telemetry_rollup_hourly
         WHERE bucket_start >= ?1 AND bucket_start < ?2
         GROUP BY model, bucket"
    );
    collect_cells(conn, &sql, window)
}

fn collect_cells(
    conn: &Connection,
    sql: &str,
    window: &HorizonWindow,
) -> rusqlite::Result<CellMap> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![window.from, window.to], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut cells: CellMap = HashMap::new();
    for row in rows {
        let (key, bucket, value) = row?;
        cells.entry(key).or_default().push((bucket, value));
    }
    Ok(cells)
}

/// Window-wide totals for several measures at once, with no time axis.
///
/// For a caller that wants figures but not a shape. [`matrix_at`] answers both,
/// and the axis is the expensive half: on a real 1.2 GB store over a trailing 30
/// days, six provider grids cost 734 ms against 234 ms for the same six totals,
/// because the cells query buckets every fact while the totals query is a plain
/// `GROUP BY` over an indexed range.
///
/// Measures sharing a fact table are answered in **one** query rather than one
/// each, which is most of the remaining difference. `ActiveMs` always gets its
/// own: it is clamped to the window rather than filtered by it, so its `WHERE`
/// cannot be shared.
///
/// The aggregates come from [`Measure::fact_expr`], the same place [`matrix_at`]
/// gets them. A surface reading totals here and cells there must not be able to
/// quote two different answers for one window.
pub fn totals_at(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measures: &[Measure],
) -> rusqlite::Result<HashMap<Measure, HashMap<String, i64>>> {
    totals_at_impl(conn, window, dimension, measures, false)
}

/// Window-wide totals after grouping verified provider-child facts under roots.
pub fn grouped_totals_at(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measures: &[Measure],
) -> rusqlite::Result<HashMap<Measure, HashMap<String, i64>>> {
    totals_at_impl(conn, window, dimension, measures, true)
}

fn totals_at_impl(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measures: &[Measure],
    grouped_agents: bool,
) -> rusqlite::Result<HashMap<Measure, HashMap<String, i64>>> {
    let grouped_agent_rows = grouped_agents && dimension == Dimension::Agent;
    let mut answers: HashMap<Measure, HashMap<String, i64>> = HashMap::new();

    // Grouped by source, in a stable order, so the emitted SQL does not depend
    // on hash iteration order and a query plan stays reproducible.
    for source in [
        MeasureSource::Turns,
        MeasureSource::Edits,
        MeasureSource::Activity,
    ] {
        let batch: Vec<Measure> = measures
            .iter()
            .copied()
            .filter(|measure| measure.source() == source)
            .collect();
        if batch.is_empty() {
            continue;
        }

        // Two combinations refuse to batch and fall back to answering one at a
        // time: active time is clamped rather than filtered, and a model view of
        // it has to come from the rollup because activity facts carry no model.
        if source == MeasureSource::Activity {
            for measure in batch {
                let needs_rollup = dimension == Dimension::Model;
                answers.insert(
                    measure,
                    row_totals(
                        conn,
                        window,
                        dimension,
                        measure,
                        needs_rollup,
                        grouped_agent_rows,
                    )?
                    .into_iter()
                    .collect(),
                );
            }
            continue;
        }

        let table = source.table();
        let time = source.time_column();
        let needs_join = dimension == Dimension::Model && source == MeasureSource::Edits;
        let (alias, join, column) = if needs_join {
            ("e", EDIT_MODEL_JOIN.to_string(), format!("e.{time}"))
        } else if grouped_agent_rows {
            ("f", agent_root_join("f"), format!("f.{time}"))
        } else {
            ("", String::new(), time.to_string())
        };
        let key = if grouped_agent_rows {
            agent_root_key(alias)
        } else {
            fact_key(dimension, source).to_string()
        };
        let exprs: Vec<String> = batch
            .iter()
            .map(|measure| {
                if grouped_agent_rows && *measure == Measure::Turns {
                    distinct_turn_expr(alias)
                } else if needs_join {
                    measure.fact_expr().replace("path", "e.path")
                } else {
                    measure.fact_expr().to_string()
                }
            })
            .collect();
        let sql = format!(
            "SELECT {key}, {}
             FROM {table} {alias} {join}
             WHERE {column} >= ?1 AND {column} < ?2
             GROUP BY {key}",
            exprs.join(", ")
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![window.from, window.to], |row| {
            let key: String = row.get(0)?;
            let values: Vec<i64> = (1..=batch.len())
                .map(|column| row.get::<_, i64>(column))
                .collect::<rusqlite::Result<_>>()?;
            Ok((key, values))
        })?;

        for row in rows {
            let (key, values) = row?;
            for (measure, value) in batch.iter().zip(values) {
                answers
                    .entry(*measure)
                    .or_default()
                    .insert(key.clone(), value);
            }
        }

        // A measure that matched no rows still has an answer: an empty map,
        // which reads as "nothing in this window" rather than as "never asked".
        for measure in batch {
            answers.entry(measure).or_default();
        }
    }

    Ok(answers)
}

/// Window-wide totals per row.
///
/// Computed separately from the cells rather than by summing them, because for
/// a distinct count the two genuinely differ.
fn row_totals(
    conn: &Connection,
    window: &HorizonWindow,
    dimension: Dimension,
    measure: Measure,
    needs_rollup: bool,
    grouped_agents: bool,
) -> rusqlite::Result<Vec<(String, i64)>> {
    let sql = if needs_rollup {
        "SELECT model, COALESCE(SUM(measured_active_ms), 0) + COALESCE(SUM(clustered_active_ms), 0)
         FROM telemetry_rollup_hourly
         WHERE bucket_start >= ?1 AND bucket_start < ?2
         GROUP BY model"
            .to_string()
    } else if measure == Measure::ActiveMs {
        let (key, join) = if grouped_agents {
            (agent_root_key("a"), agent_root_join("a"))
        } else {
            (
                match dimension {
                    Dimension::Provider => "provider".to_string(),
                    _ => "session_id".to_string(),
                },
                String::new(),
            )
        };
        // Clamped to the window so a span crossing the edge contributes only
        // the part inside it, matching what the cells drew.
        format!(
            "SELECT {key}, COALESCE(SUM(
                 (MIN(strftime('%s', a.ended_at), strftime('%s', ?2))
                  - MAX(strftime('%s', a.started_at), strftime('%s', ?1))) * 1000), 0)
             FROM telemetry_activity a {join}
             WHERE a.ended_at > ?1 AND a.started_at < ?2
             GROUP BY {key}"
        )
    } else {
        let source = measure.source();
        let table = source.table();
        let time = source.time_column();
        let needs_join = dimension == Dimension::Model && source == MeasureSource::Edits;
        let (alias, join, column) = if needs_join {
            ("e", EDIT_MODEL_JOIN.to_string(), format!("e.{time}"))
        } else if grouped_agents {
            ("f", agent_root_join("f"), format!("f.{time}"))
        } else {
            ("", String::new(), time.to_string())
        };
        let expr = if grouped_agents && measure == Measure::Turns {
            distinct_turn_expr(alias)
        } else if needs_join {
            measure.fact_expr().replace("path", "e.path")
        } else {
            measure.fact_expr().to_string()
        };
        let key = if grouped_agents {
            agent_root_key(alias)
        } else {
            fact_key(dimension, source).to_string()
        };
        format!(
            "SELECT {key}, {expr}
             FROM {table} {alias} {join}
             WHERE {column} >= ?1 AND {column} < ?2
             GROUP BY {key}"
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![window.from, window.to], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::horizon::{resolve_horizon, Horizon};
    use crate::telemetry::ingest::ingest_source;
    use crate::telemetry::schema::run_telemetry_migrations;
    use crate::telemetry::sources::SourceContext;

    fn fixture() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("codex-rollout.jsonl")
    }

    fn ingested() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_telemetry_migrations(&conn).unwrap();
        let ctx = SourceContext::new("agent-1", "codex", &fixture());
        ingest_source(&conn, &ctx).unwrap();
        conn
    }

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

    /// A store holding hand-written turns, so a measure's arithmetic can be
    /// pinned against numbers chosen to make a wrong answer obvious.
    fn store_with_turns(turns: &[(&str, i64, i64, i64, i64)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_telemetry_migrations(&conn).unwrap();
        for (index, (ended_at, input, cached, cache_write, output)) in turns.iter().enumerate() {
            conn.execute(
                "INSERT INTO telemetry_turns
                   (event_key, session_id, provider, model, ended_at, input_tokens,
                    cached_input_tokens, cache_write_tokens, output_tokens,
                    source_key, source_path)
                 VALUES (?1, 'agent-1', 'claude', 'claude-sonnet-5', ?2, ?3, ?4, ?5, ?6, 'k', 'p')",
                params![
                    format!("e{index}"),
                    ended_at,
                    input,
                    cached,
                    cache_write,
                    output
                ],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn the_total_counts_cache_writes_and_still_excludes_cache_reads() {
        // The real 400-turn claude session these figures come from: claude sends
        // almost nothing as plain input, so a total of input + output alone
        // reports 471,400 for work that processed 2,357,271 tokens.
        let conn = store_with_turns(&[(
            "2026-08-13T15:00:00.000Z",
            8_446,
            80_194_623,
            1_885_871,
            462_954,
        )]);
        let total = |measure| {
            matrix_at(
                &conn,
                &window(),
                Dimension::Agent,
                measure,
                usize::MAX,
                None,
            )
            .unwrap()
            .rows[0]
                .total
        };

        assert_eq!(total(Measure::TotalTokens), 2_357_271);
        assert_eq!(total(Measure::CacheWriteTokens), 1_885_871);
        // Neither the old figure nor the sum of every component.
        assert_ne!(total(Measure::TotalTokens), 471_400);
        assert_ne!(total(Measure::TotalTokens), 82_551_894);
    }

    #[test]
    fn the_hit_rate_is_recomputed_for_row_totals_rather_than_averaged() {
        // Two buckets with very different hit rates and very different volumes.
        // The mean of the cells is 50%; the real rate over the window is 99%,
        // because the busy bucket dwarfs the quiet one. Averaging cells would
        // let one idle bucket rewrite the window's answer.
        let conn = store_with_turns(&[
            ("2026-08-13T14:30:00.000Z", 100, 0, 0, 0),
            ("2026-08-13T15:30:00.000Z", 100, 99_900, 0, 0),
        ]);
        let matrix = matrix_at(
            &conn,
            &window(),
            Dimension::Agent,
            Measure::CacheHitRate,
            usize::MAX,
            None,
        )
        .unwrap();

        let row = &matrix.rows[0];
        assert_eq!(row.total, 99);
        let mean: i64 = row.cells.iter().sum::<i64>() / row.cells.len() as i64;
        assert_ne!(row.total, mean);
        // And the surface must be told not to present the cells as summing.
        assert!(matrix.cells_are_not_additive);
    }

    #[test]
    fn a_window_with_no_prompt_tokens_has_a_hit_rate_of_zero_not_an_error() {
        let conn = store_with_turns(&[("2026-08-13T15:00:00.000Z", 0, 0, 0, 12)]);
        let matrix = matrix_at(
            &conn,
            &window(),
            Dimension::Agent,
            Measure::CacheHitRate,
            usize::MAX,
            None,
        )
        .unwrap();
        assert_eq!(matrix.rows[0].total, 0);
    }

    #[test]
    fn batched_totals_agree_with_the_matrix_they_skip_the_axis_for() {
        // The whole point of `totals_at` is to be cheaper, not different. A
        // surface reading totals here and cells from `matrix_at` must never be
        // able to quote two answers for one window, so this pins every measure
        // against the function it is an optimisation of.
        let conn = ingested();
        let window = window();

        for dimension in [Dimension::Agent, Dimension::Provider, Dimension::Model] {
            let batched = totals_at(&conn, &window, dimension, &EVERY_MEASURE).unwrap();

            for measure in EVERY_MEASURE {
                let expected: HashMap<String, i64> =
                    matrix_at(&conn, &window, dimension, measure, usize::MAX, None)
                        .unwrap()
                        .rows
                        .into_iter()
                        .map(|row| (row.key, row.total))
                        .collect();
                assert_eq!(
                    batched[&measure], expected,
                    "{dimension:?} / {measure:?} disagreed"
                );
            }
        }
    }

    #[test]
    fn a_measure_with_no_rows_is_answered_as_empty_rather_than_missing() {
        // "Nothing in this window" and "never asked" are different claims, and
        // a caller indexing the map must not have to tell them apart.
        let conn = Connection::open_in_memory().unwrap();
        run_telemetry_migrations(&conn).unwrap();

        let batched = totals_at(&conn, &window(), Dimension::Provider, &EVERY_MEASURE).unwrap();
        for measure in EVERY_MEASURE {
            assert!(batched[&measure].is_empty(), "{measure:?} was missing");
        }
    }

    #[test]
    fn measures_sharing_a_fact_table_are_answered_together() {
        // Batching is the optimisation; asking for one measure per table must
        // still return every one of them, or the saving comes from dropping
        // answers rather than from dropping work.
        let conn = ingested();
        let batched = totals_at(
            &conn,
            &window(),
            Dimension::Provider,
            &[Measure::Turns, Measure::TotalTokens, Measure::Files],
        )
        .unwrap();

        assert_eq!(batched.len(), 3);
        assert!(batched[&Measure::Turns].values().any(|total| *total > 0));
    }

    /// A two-hour window containing the fixture, resolving to 5-minute columns.
    fn window() -> HorizonWindow {
        HorizonWindow {
            from: "2026-08-13T14:00:00.000Z".into(),
            to: "2026-08-13T16:00:00.000Z".into(),
            from_floored: false,
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn a_bucket_cap_coarsens_short_windows_without_flattening_them() {
        // The reason the cap is a bucket count and not a fixed width: a fixed
        // hourly floor would draw a four-hour horizon as four columns, which is
        // a bar chart with no shape rather than a sparkline.
        let (from, to) = (at("2026-08-13T12:00:00Z"), at("2026-08-13T16:00:00Z"));
        assert_eq!(Grain::for_window(from, to), Grain::Minute5);
        assert_eq!(Grain::for_window_within(from, to, 48), Grain::Minute5);
        assert_eq!(
            Grain::for_window_within(from, to, 48).bucket_count(from, to),
            48
        );
    }

    #[test]
    fn a_bucket_cap_is_honoured_across_every_horizon() {
        for (from, to) in [
            ("2026-08-13T12:00:00Z", "2026-08-13T16:00:00Z"),
            ("2026-08-12T16:00:00Z", "2026-08-13T16:00:00Z"),
            ("2026-08-06T16:00:00Z", "2026-08-13T16:00:00Z"),
            ("2026-07-14T16:00:00Z", "2026-08-13T16:00:00Z"),
            ("2025-08-13T16:00:00Z", "2026-08-13T16:00:00Z"),
        ] {
            let (from, to) = (at(from), at(to));
            let grain = Grain::for_window_within(from, to, 48);
            let count = grain.bucket_count(from, to);
            assert!(
                count <= 48 || grain == Grain::Day,
                "{grain:?} produced {count} buckets"
            );
        }
    }

    #[test]
    fn a_bucket_count_matches_the_axis_actually_laid_out() {
        // The axis starts at the grain boundary below `from`, so an unaligned
        // window covers one more column than its length divided by the width.
        // Counting by division reported 48 where 49 were drawn, and the cap the
        // Dashboard relies on was quietly exceeded.
        let (from, to) = (at("2026-08-13T12:02:00Z"), at("2026-08-13T16:02:00Z"));
        assert_eq!(
            Grain::Minute5.bucket_count(from, to),
            bucket_bounds(from, to, Grain::Minute5).len()
        );
        assert_eq!(Grain::Minute5.bucket_count(from, to), 49);
    }

    #[test]
    fn a_cap_is_honoured_for_a_window_that_does_not_start_on_a_boundary() {
        let (from, to) = (at("2026-08-13T12:02:00Z"), at("2026-08-13T16:02:00Z"));
        let grain = Grain::for_window_within(from, to, 48);
        assert!(bucket_bounds(from, to, grain).len() <= 48);
    }

    #[test]
    fn a_cap_never_refines_a_window_that_already_fits() {
        // Asking for "at most fifty" over a year must not turn days into hours.
        let (from, to) = (at("2025-08-13T16:00:00Z"), at("2026-08-13T16:00:00Z"));
        assert_eq!(Grain::for_window_within(from, to, 500), Grain::Day);
    }

    #[test]
    fn measure_names_round_trip_and_reject_anything_else() {
        for measure in [
            Measure::ActiveMs,
            Measure::Turns,
            Measure::FreshTokens,
            Measure::CachedTokens,
            Measure::OutputTokens,
            Measure::ReasoningTokens,
            Measure::TotalTokens,
            Measure::Files,
            Measure::LinesAdded,
            Measure::LinesRemoved,
            Measure::LinesChanged,
        ] {
            assert_eq!(Measure::parse(measure.as_str()), Some(measure));
        }
        // Selects a SQL expression, so it must be closed like `Dimension`.
        assert_eq!(Measure::parse("SUM(1); DROP TABLE telemetry_turns"), None);
        assert_eq!(Measure::parse("cost"), None);
    }

    #[test]
    fn a_short_window_gets_fine_columns_rather_than_a_few_blocks() {
        // The defect this replaces: the grain bottomed out at an hour, so a
        // four-hour window rendered as five enormous blocks and erased every
        // distinction inside them.
        let from = parse_instant("2026-08-14T00:00:00.000Z");
        let at = parse_instant;
        assert_eq!(
            Grain::for_window(from, at("2026-08-14T04:00:00.000Z")),
            Grain::Minute5
        );
        assert_eq!(
            Grain::for_window(from, at("2026-08-14T20:00:00.000Z")),
            Grain::Minute15
        );
        assert_eq!(
            Grain::for_window(from, at("2026-08-17T00:00:00.000Z")),
            Grain::Hour
        );
        assert_eq!(
            Grain::for_window(from, at("2026-09-05T00:00:00.000Z")),
            Grain::SixHour
        );
        assert_eq!(
            Grain::for_window(from, at("2027-01-01T00:00:00.000Z")),
            Grain::Day
        );

        let today = HorizonWindow {
            from: "2026-08-14T00:00:00.000Z".into(),
            to: "2026-08-14T04:00:00.000Z".into(),
            from_floored: false,
        };
        let grid = matrix(&ingested(), &today, Dimension::Agent, Measure::ActiveMs, 40).unwrap();
        assert_eq!(grid.grain, Grain::Minute5);
        assert_eq!(
            grid.buckets.len(),
            48,
            "a four-hour today has 48 columns, not 5"
        );
    }

    #[test]
    fn the_axis_is_dense_including_buckets_where_nothing_happened() {
        let grid = matrix(
            &ingested(),
            &window(),
            Dimension::Agent,
            Measure::TotalTokens,
            24,
        )
        .unwrap();
        assert_eq!(grid.grain, Grain::Minute5);
        assert_eq!(grid.buckets.len(), 24);
        assert_eq!(grid.buckets[0], "2026-08-13T14:00:00.000Z");
        assert_eq!(grid.buckets[1], "2026-08-13T14:05:00.000Z");
        assert!(grid
            .rows
            .iter()
            .all(|r| r.cells.len() == grid.buckets.len()));
    }

    #[test]
    fn cells_carry_the_selected_measure() {
        let grid = matrix(
            &ingested(),
            &window(),
            Dimension::Agent,
            Measure::TotalTokens,
            24,
        )
        .unwrap();
        let row = &grid.rows[0];
        assert_eq!(row.key, "agent-1");
        // Fresh input 100,544 plus output 5,254.
        assert_eq!(row.total, 105_798);
        assert_eq!(row.cells.iter().sum::<i64>(), row.total);
        assert!(!grid.cells_are_not_additive);
    }

    #[test]
    fn an_activity_span_is_split_across_the_buckets_it_crosses() {
        // Grouping a span by its start would drop a long interval entirely into
        // its first cell, which is the resolution loss this view exists to
        // avoid. It has to be distributed by overlap instead.
        let conn = ingested();
        conn.execute("DELETE FROM telemetry_activity", []).unwrap();
        conn.execute(
            "INSERT INTO telemetry_activity
                (session_id, provider, source_key, started_at, ended_at, last_event_at,
                 event_count, method)
             VALUES ('agent-1', 'codex', 'src', '2026-08-13T14:02:00.000Z',
                     '2026-08-13T14:18:00.000Z', '2026-08-13T14:18:00.000Z', 5, 'clustered')",
            [],
        )
        .unwrap();

        let grid = matrix(&conn, &window(), Dimension::Agent, Measure::ActiveMs, 24).unwrap();
        let row = &grid.rows[0];
        let occupied: Vec<usize> = row
            .cells
            .iter()
            .enumerate()
            .filter(|(_, value)| **value > 0)
            .map(|(index, _)| index)
            .collect();

        // 14:02-14:18 touches the 14:00, 14:05, 14:10 and 14:15 buckets.
        assert_eq!(occupied, vec![0, 1, 2, 3]);
        assert_eq!(
            row.cells[0],
            3 * 60_000,
            "only the tail of the first bucket"
        );
        assert_eq!(row.cells[1], 5 * 60_000, "a fully covered bucket");
        assert_eq!(row.cells[3], 3 * 60_000, "only the head of the last bucket");
        assert_eq!(row.cells.iter().sum::<i64>(), 16 * 60_000);
        assert_eq!(row.total, 16 * 60_000);
    }

    #[test]
    fn an_activity_span_crossing_the_window_edge_counts_only_the_part_inside() {
        let conn = ingested();
        conn.execute("DELETE FROM telemetry_activity", []).unwrap();
        conn.execute(
            "INSERT INTO telemetry_activity
                (session_id, provider, source_key, started_at, ended_at, last_event_at,
                 event_count, method)
             VALUES ('agent-1', 'codex', 'src', '2026-08-13T13:50:00.000Z',
                     '2026-08-13T14:10:00.000Z', '2026-08-13T14:10:00.000Z', 5, 'clustered')",
            [],
        )
        .unwrap();

        let grid = matrix(&conn, &window(), Dimension::Agent, Measure::ActiveMs, 24).unwrap();
        let row = &grid.rows[0];
        // Ten of the twenty minutes fall inside the window, and the total has to
        // agree with the cells rather than reporting the whole span.
        assert_eq!(row.cells.iter().sum::<i64>(), 10 * 60_000);
        assert_eq!(row.total, 10 * 60_000);
    }

    #[test]
    fn a_distinct_count_total_is_not_the_sum_of_its_cells() {
        let grid = matrix(&ingested(), &window(), Dimension::Agent, Measure::Turns, 24).unwrap();
        let row = &grid.rows[0];
        assert_eq!(row.total, 2, "two real turns in the window");
        assert!(row.cells.iter().sum::<i64>() > row.total);
        assert!(grid.cells_are_not_additive);
    }

    #[test]
    fn rows_are_ordered_by_total_with_a_stable_tiebreak() {
        let conn = ingested();
        conn.execute(
            "INSERT INTO telemetry_edits
                (event_key, source_key, source_path, session_id, provider, occurred_at,
                 path, op, lines_added, lines_removed)
             VALUES ('k-small', 'src', 'p', 'agent-0', 'codex',
                     '2026-08-13T14:10:00.000Z', 'D:/x.rs', 'update', 1, 0)",
            [],
        )
        .unwrap();

        let grid = matrix(
            &conn,
            &window(),
            Dimension::Agent,
            Measure::LinesChanged,
            24,
        )
        .unwrap();
        assert_eq!(grid.rows[0].key, "agent-1");
        assert_eq!(grid.rows[1].key, "agent-0");
        assert!(grid.rows[0].total > grid.rows[1].total);

        // Truncation must drop the smallest, not whichever SQL returned last.
        let capped = matrix(&conn, &window(), Dimension::Agent, Measure::LinesChanged, 1).unwrap();
        assert_eq!(capped.rows.len(), 1);
        assert_eq!(capped.rows[0].key, "agent-1");
    }

    #[test]
    fn an_empty_window_is_an_axis_with_no_rows() {
        // The fresh-install state: a drawable empty grid, not an error and not
        // a grid with no columns.
        let empty = HorizonWindow {
            from: "2020-01-01T00:00:00.000Z".into(),
            to: "2020-01-01T02:00:00.000Z".into(),
            from_floored: false,
        };
        let grid = matrix(&ingested(), &empty, Dimension::Agent, Measure::ActiveMs, 24).unwrap();
        assert_eq!(grid.buckets.len(), 24);
        assert!(grid.rows.is_empty());
        assert_eq!(grid.max_cell, 0);
    }

    #[test]
    fn a_long_horizon_coarsens_rather_than_generating_an_unbounded_axis() {
        let conn = ingested();
        let month = resolve_horizon(Horizon::Month, Utc::now());
        let grid = matrix(&conn, &month, Dimension::Agent, Measure::ActiveMs, 24).unwrap();
        assert_eq!(grid.grain, Grain::SixHour);
        assert!(grid.buckets.len() <= MAX_BUCKETS);

        let all = resolve_horizon(Horizon::All, Utc::now());
        let grid = matrix(&conn, &all, Dimension::Agent, Measure::ActiveMs, 24).unwrap();
        assert_eq!(grid.grain, Grain::Day);
        assert!(grid.buckets.len() <= MAX_BUCKETS);
    }

    #[test]
    fn every_dimension_and_measure_produces_a_grid() {
        let conn = ingested();
        for dimension in [Dimension::Provider, Dimension::Agent, Dimension::Model] {
            for measure in [
                Measure::ActiveMs,
                Measure::Turns,
                Measure::TotalTokens,
                Measure::Files,
                Measure::LinesChanged,
            ] {
                let grid = matrix(&conn, &window(), dimension, measure, 24).unwrap();
                assert!(
                    grid.rows
                        .iter()
                        .all(|r| r.cells.len() == grid.buckets.len()),
                    "{dimension:?}/{measure:?} produced a ragged grid"
                );
            }
        }
    }

    #[test]
    fn active_time_by_model_falls_back_to_the_rollups_grain() {
        // Activity facts carry no model, so this one combination cannot be
        // answered at fine resolution. Falling back is honest; fabricating an
        // attribution would not be.
        let grid = matrix(
            &ingested(),
            &window(),
            Dimension::Model,
            Measure::ActiveMs,
            24,
        )
        .unwrap();
        assert_eq!(grid.grain, Grain::Hour);
        assert!(!grid.rows.is_empty());
    }

    #[test]
    fn an_edit_is_attributed_to_exactly_one_model_even_if_its_turn_names_two() {
        // Codex writes several token records per turn_id and they normally agree
        // on the model, but nothing enforces it. Joining the turn facts directly
        // would emit the edit once per model and report two files where one was
        // touched.
        let conn = ingested();
        conn.execute(
            "INSERT INTO telemetry_turns
                (event_key, source_key, source_path, session_id, provider, turn_id, model, ended_at)
             VALUES ('k-alt', 'src', 'p', 'agent-1', 'codex', 'turn-001',
                     'a-different-model', '2026-08-13T14:50:00.000Z')",
            [],
        )
        .unwrap();

        let grid = matrix(&conn, &window(), Dimension::Model, Measure::Files, 24).unwrap();
        let attributed: i64 = grid.rows.iter().map(|r| r.total).sum();
        let distinct: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT path) FROM telemetry_edits",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            attributed, distinct,
            "a turn naming two models must not double-count files"
        );
    }

    #[test]
    fn a_distinct_count_does_not_sum_across_rows_either() {
        // The row-wise counterpart of `cells_are_not_additive`, and a property
        // rather than a defect: one file edited by two agents is one file
        // overall and belongs in both rows. Pinned so nobody later makes the
        // rows add up, and so no surface offers a column total.
        let conn = ingested();
        conn.execute(
            "INSERT INTO telemetry_edits
                (event_key, source_key, source_path, session_id, provider, occurred_at, path, op)
             SELECT 'k-shared', 'src', 'p', 'agent-2', 'codex', occurred_at, path, op
             FROM telemetry_edits LIMIT 1",
            [],
        )
        .unwrap();

        let grid = matrix(&conn, &window(), Dimension::Agent, Measure::Files, 24).unwrap();
        let summed: i64 = grid.rows.iter().map(|r| r.total).sum();
        let distinct: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT path) FROM telemetry_edits",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(grid.rows.len(), 2);
        assert!(
            summed > distinct,
            "a shared file belongs to both rows, so rows over-sum"
        );
    }
}
