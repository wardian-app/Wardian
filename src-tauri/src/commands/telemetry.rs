//! Read commands over the habitat telemetry store.
//!
//! Every command here answers from `telemetry_rollup_hourly` by way of
//! [`wardian_core::telemetry::query`], so a horizon costs one row per hour per
//! agent per model rather than a re-read of the provider logs.
//!
//! Commands take a named horizon rather than raw timestamps, and resolve it
//! through [`wardian_core::telemetry::horizon`]. Buckets are hourly, so a
//! caller-supplied `from` that lands mid-hour would silently drop that whole
//! hour — the surface would show a figure that was simply wrong rather than one
//! it could recognize as truncated. Keeping the rule in the core means the CLI
//! answers the same question the same way.

use crate::state::AppState;
use chrono::Utc;
use serde::Serialize;
use tauri::State;
use wardian_core::telemetry::attribution::token_reporting_agents;
// Horizon resolution lives in the core so the CLI resolves "the last 24 hours"
// the same way this does. Two independent implementations of the flooring rule
// would let two surfaces quote different figures for the same question.
use wardian_core::telemetry::horizon::{resolve_horizon, Horizon, HorizonWindow};
use wardian_core::telemetry::matrix::{
    grouped_matrix_at, grouped_totals_at, matrix_at, totals_at, Measure,
};
use wardian_core::telemetry::models::{
    ActiveTime, BreakdownRow, IntervalFact, LimitObservation, TelemetrySummary, TokenCounts,
};
use wardian_core::telemetry::query::{
    activity_intervals, breakdown, grouped_breakdown, grouped_series, latest_limits, series,
    summary, Dimension, SeriesPoint,
};

/// A breakdown row with the label a surface should actually print.
///
/// `key` stays the store's own value so the UI can correlate rows across calls;
/// `label` is what a person reads. For agents these differ — the store keys on a
/// session UUID, which is meaningless on screen.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryBreakdownRowDto {
    pub key: String,
    pub label: String,
    pub active: ActiveTime,
    pub turns: i64,
    pub tokens: TokenCounts,
    pub processed_tokens: Option<i64>,
    pub files_touched: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub agent_count: i64,
    pub tokens_reported: bool,
}

/// One agent's figures for a horizon, several measures at once.
///
/// The Dashboard's unit. Deliberately wide rather than one measure at a time:
/// the question is almost always "who has been doing what lately", and
/// answering it from separate per-measure lists makes the reader join them by
/// hand.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryAgentRowDto {
    pub key: String,
    pub label: String,
    pub sublabel: Option<String>,
    /// Measured and inferred durations summed. The split is preserved in the
    /// store but is not worth a column.
    pub active_ms: i64,
    pub turns: i64,
    /// New content processed: fresh input, cache writes, and output. Cache
    /// reads are excluded; see `TokenCounts::processed_total`.
    pub total_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub files_touched: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    /// False when no contributing provider reported token accounting, so the
    /// row reads as unmeasured rather than as zero.
    pub tokens_reported: bool,
    /// True when this agent recorded nothing in the window. It is still listed,
    /// because a roster that hides its quiet members answers a different
    /// question from the one being asked.
    pub idle: bool,
    /// The selected measure bucketed across the window, one value per entry in
    /// [`TelemetryDashboardDto::buckets`].
    ///
    /// A row's figures are a total, and a total cannot tell a steady week from
    /// one frantic afternoon. This is what makes the Dashboard answer "across
    /// time" without leaving the surface.
    pub spark: Vec<i64>,
}

/// One provider's contribution to the habitat, plus its account headroom.
///
/// Capacity lives here rather than in a component of its own so that the
/// Dashboard has the same shape whatever a habitat runs. A provider that
/// publishes no limit reports `limits: []`, which reads as "not reported" beside
/// providers that do — the same treatment every other unreported measure gets,
/// instead of the element vanishing.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryProviderRowDto {
    pub provider: String,
    /// How many agents on this provider recorded anything in the window.
    pub agent_count: i64,
    pub active_ms: i64,
    pub turns: i64,
    pub total_tokens: Option<i64>,
    pub files_touched: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    /// False when this provider publishes no token accounting at all.
    pub tokens_reported: bool,
    /// Account gauges for this provider. Never per-agent: two agents on one
    /// account observe the same figure, so these are never summed.
    pub limits: Vec<LimitObservation>,
}

/// The Dashboard payload: every agent, plus account-level state.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryDashboardDto {
    pub window: HorizonWindow,
    pub rows: Vec<TelemetryAgentRowDto>,
    /// One row per provider the habitat actually used in this window.
    ///
    /// A structural element, not a conditional one. Account headroom used to be
    /// its own component, which meant the Dashboard grew and lost a whole block
    /// depending on which provider happened to be in use — codex publishes a
    /// limit and nothing else does, so the layout was effectively codex-shaped.
    /// Capacity is now a *field* on a provider row, absent the way any other
    /// unreported measure is absent.
    pub providers: Vec<TelemetryProviderRowDto>,
    /// Bucket start instants for every row's `spark`, ascending.
    pub buckets: Vec<String>,
    /// Which measure the sparklines carry.
    pub spark_measure: String,
    /// Bucket width, so a surface can say what one spark column covers.
    pub grain: String,
    /// Largest single bucket across every row, so sparklines share one scale
    /// and a busy agent looks busier than a quiet one instead of every row
    /// being normalized to its own maximum.
    pub spark_max: i64,
}

/// One row of the matrix, carrying the name a person reads.
///
/// `label` is separate from `key` because the store keys agents on a session
/// UUID. Rendering the key is what put raw UUIDs down the side of the first
/// Dashboard; every row that reaches a surface goes through [`label_for`].
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryMatrixRowDto {
    pub key: String,
    pub label: String,
    /// Secondary line — an agent's class, or the provider behind a model.
    pub sublabel: Option<String>,
    pub cells: Vec<i64>,
    pub total: i64,
}

/// A dense rows × buckets grid for one measure.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryMatrixDto {
    pub dimension: String,
    pub measure: String,
    pub grain: String,
    pub window: HorizonWindow,
    pub buckets: Vec<String>,
    pub rows: Vec<TelemetryMatrixRowDto>,
    pub max_cell: i64,
    /// True when the measure is a distinct count, so a row's cells do not sum
    /// to its total and a surface must not present them as if they did.
    pub cells_are_not_additive: bool,
}

/// Everything the Dashboard needs for one horizon, in one round trip.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryOverviewDto {
    pub window: HorizonWindow,
    pub summary: TelemetrySummary,
    /// New content processed: fresh input, cache writes, and output. Computed
    /// once here so every surface shows the same figure rather than each
    /// re-deriving what "tokens used" means.
    pub processed_tokens: Option<i64>,
    /// True when measured and clustered durations both contributed, so any
    /// single active-time figure shown is a mixture of a measurement and an
    /// estimate and has to be labelled as one.
    pub active_is_mixed: bool,
    pub by_provider: Vec<TelemetryBreakdownRowDto>,
    pub by_agent: Vec<TelemetryBreakdownRowDto>,
    pub by_model: Vec<TelemetryBreakdownRowDto>,
    pub limits: Vec<LimitObservation>,
}

/// How many rows a breakdown returns before the tail is dropped.
const BREAKDOWN_LIMIT: usize = 24;

/// How many columns a Dashboard sparkline may have.
///
/// The Analytics matrix aims for roughly a hundred because it is an axis values
/// get read off. A sparkline is a shape glanced at inside a table row, so it
/// gets a coarser grain — chosen by bucket count rather than fixed width, so
/// that a four-hour horizon still resolves to five-minute columns instead of
/// four hourly ones.
const SPARK_BUCKETS: usize = 48;

/// Every read here is `#[tauri::command(async)]` on a synchronous body.
///
/// That combination is deliberate. A plain `#[tauri::command]` on a sync
/// function runs on the **main thread**, and these reads take the global
/// database mutex — which an ingest pass can hold for as long as its timeout
/// allows while it works through a large backlog. On the main thread that is a
/// frozen window, not a slow query. `(async)` moves the same synchronous body
/// onto the runtime's blocking pool, where waiting for the mutex costs nothing
/// but the caller's own latency.
#[tauri::command(async)]
pub fn telemetry_overview(horizon: String) -> Result<TelemetryOverviewDto, String> {
    let horizon =
        Horizon::parse(&horizon).ok_or_else(|| format!("unknown telemetry horizon: {horizon}"))?;
    let window = resolve_horizon(horizon, Utc::now());
    let labels = agent_labels();

    wardian_core::db::get_db_conn(|conn| {
        let summary = summary(conn, &window.from, &window.to)?;
        let rows = |dimension| -> Result<Vec<BreakdownRow>, Box<dyn std::error::Error>> {
            Ok(breakdown(
                conn,
                dimension,
                &window.from,
                &window.to,
                BREAKDOWN_LIMIT,
            )?)
        };
        Ok(TelemetryOverviewDto {
            processed_tokens: summary.tokens.processed_total(),
            active_is_mixed: summary.active.is_mixed(),
            by_provider: to_dtos(rows(Dimension::Provider)?, Dimension::Provider, &labels),
            by_agent: to_dtos(
                grouped_breakdown(
                    conn,
                    Dimension::Agent,
                    &window.from,
                    &window.to,
                    BREAKDOWN_LIMIT,
                )?,
                Dimension::Agent,
                &labels,
            ),
            by_model: to_dtos(rows(Dimension::Model)?, Dimension::Model, &labels),
            limits: latest_limits(conn)?,
            summary,
            window,
        })
    })
    .map_err(|error| format!("could not read telemetry overview: {error}"))
}

/// Every agent's figures for a horizon, for the Dashboard.
///
/// See [`telemetry_overview`] for why this is `(async)`.
#[tauri::command(async)]
pub fn telemetry_dashboard(
    horizon: String,
    measure: Option<String>,
) -> Result<TelemetryDashboardDto, String> {
    let horizon =
        Horizon::parse(&horizon).ok_or_else(|| format!("unknown telemetry horizon: {horizon}"))?;
    let window = resolve_horizon(horizon, Utc::now());
    let labels = agent_labels();
    let spark_measure = match measure.as_deref() {
        None => Measure::ActiveMs,
        Some(name) => {
            Measure::parse(name).ok_or_else(|| format!("unknown telemetry measure: {name}"))?
        }
    };

    wardian_core::db::get_db_conn(|conn| {
        // No practical limit: the roster is the answer, so truncating it would
        // silently drop agents from a view whose job is to list them.
        let measured =
            grouped_breakdown(conn, Dimension::Agent, &window.from, &window.to, usize::MAX)?;

        // One grid for every agent's sparkline, rather than a query per row.
        let grid = grouped_matrix_at(
            conn,
            &window,
            Dimension::Agent,
            spark_measure,
            usize::MAX,
            Some(SPARK_BUCKETS),
        )?;
        let bucket_count = grid.buckets.len();
        let sparks: std::collections::HashMap<String, Vec<i64>> = grid
            .rows
            .into_iter()
            .map(|row| (row.key, row.cells))
            .collect();

        let mut rows: Vec<TelemetryAgentRowDto> = measured
            .into_iter()
            .map(|row| TelemetryAgentRowDto {
                spark: sparks
                    .get(&row.key)
                    .cloned()
                    .unwrap_or_else(|| vec![0; bucket_count]),
                label: label_for(&row.key, Dimension::Agent, &labels),
                sublabel: sublabel_for(&row.key, Dimension::Agent, &labels),
                active_ms: row.active.measured_ms + row.active.clustered_ms,
                turns: row.turns,
                total_tokens: row.tokens.processed_total(),
                cached_tokens: row.tokens.cached_input_tokens,
                files_touched: row.files_touched,
                lines_added: row.lines_added,
                lines_removed: row.lines_removed,
                tokens_reported: row.tokens_reported,
                idle: false,
                key: row.key,
            })
            .collect();

        // Seed the agents that recorded nothing. An agent quiet this week is a
        // fact about the week, and dropping its row makes the roster look
        // smaller than the habitat actually is — which is exactly how the first
        // attempt showed four agents out of fifty-four.
        let seen: std::collections::HashSet<&str> =
            rows.iter().map(|row| row.key.as_str()).collect();
        let idle: Vec<TelemetryAgentRowDto> = labels
            .iter()
            .filter(|(key, _)| !seen.contains(key.as_str()))
            .map(|(key, label)| TelemetryAgentRowDto {
                key: key.clone(),
                label: label.name.clone(),
                sublabel: label.class.clone(),
                active_ms: 0,
                turns: 0,
                // Nothing recorded is not a report of zero tokens.
                total_tokens: None,
                cached_tokens: None,
                files_touched: 0,
                lines_added: 0,
                lines_removed: 0,
                tokens_reported: false,
                idle: true,
                spark: vec![0; bucket_count],
            })
            .collect();
        rows.extend(idle);

        // One scale across the whole table. Normalizing each row to its own
        // maximum would draw a agent that ran for ten minutes and one that ran
        // all week as the same shape.
        let spark_max = rows
            .iter()
            .flat_map(|row| row.spark.iter().copied())
            .max()
            .unwrap_or(0);

        // Account gauges are keyed by provider, so they attach to the provider
        // row rather than floating beside the table with nothing to belong to.
        let all_limits = latest_limits(conn)?;
        let providers: Vec<TelemetryProviderRowDto> = breakdown(
            conn,
            Dimension::Provider,
            &window.from,
            &window.to,
            usize::MAX,
        )?
        .into_iter()
        .map(|row| TelemetryProviderRowDto {
            active_ms: row.active.measured_ms + row.active.clustered_ms,
            turns: row.turns,
            total_tokens: row.tokens.processed_total(),
            files_touched: row.files_touched,
            lines_added: row.lines_added,
            lines_removed: row.lines_removed,
            tokens_reported: row.tokens_reported,
            agent_count: row.agent_count,
            limits: all_limits
                .iter()
                .filter(|limit| limit.provider == row.key)
                .cloned()
                .collect(),
            provider: row.key,
        })
        .collect();

        Ok(TelemetryDashboardDto {
            providers,
            rows,
            window,
            buckets: grid.buckets,
            spark_measure: spark_measure.as_str().to_string(),
            grain: grid.grain.as_str().to_string(),
            spark_max,
        })
    })
    .map_err(|error| format!("could not read telemetry dashboard: {error}"))
}

/// One agent's live rates over the Dashboard's trailing window.
///
/// Rates rather than totals, deliberately. The Dashboard is a process viewer,
/// and a process viewer shows consumption *now*, not consumption since boot: a
/// cumulative figure ranks history, where a rate makes a runaway obvious.
/// Totals are still carried for the columns denominated that way, and their
/// header names the window they cover.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryFleetRowDto {
    pub key: String,
    pub label: String,
    pub sublabel: Option<String>,
    /// Billable tokens per hour. `None` when the provider reports no tokens.
    pub tokens_per_hour: Option<f64>,
    pub turns_per_hour: f64,
    pub active_ms: i64,
    pub turns: i64,
    pub total_tokens: Option<i64>,
    pub files_touched: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub tokens_reported: bool,
    /// True when nothing was recorded in the window. Still listed: on a resource
    /// monitor an idle agent is available capacity, which is the answer to
    /// "where can I spend what is left".
    pub idle: bool,
    /// The trend measure per bucket, aligned to [`TelemetryFleetDto::buckets`].
    pub spark: Vec<i64>,
}

/// The largest value in each column across the whole table.
///
/// Every scaled visual normalizes against these rather than against its own row.
/// Most columns have no absolute ceiling — only codex publishes a limit — so on
/// a fleet monitor **the fleet is the denominator**. Spotting a runaway does not
/// need a ceiling, it needs an outlier.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TelemetryFleetMaximaDto {
    pub tokens_per_hour: f64,
    pub turns_per_hour: f64,
    pub turns: i64,
    pub active_ms: i64,
    pub total_tokens: i64,
    pub files_touched: i64,
    pub lines: i64,
    pub spark: i64,
}

/// One provider's contribution over the Dashboard's trailing window.
///
/// A card in the strip above the table, not a row in it. A provider has no
/// status, no CPU, and is never "spinning", so the runaway detector the table
/// exists for has no meaning at this granularity — putting providers in the same
/// table would make every column mean two things depending on which kind of row
/// was being read.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TelemetryFleetProviderDto {
    /// The provider's own name. `"all"` on the habitat card.
    pub provider: String,
    /// Configured agents naming this provider, whatever they did in the window.
    ///
    /// The **ordering key**, and deliberately window-independent: ordering by
    /// in-window activity moves cards sideways whenever the window setting
    /// changes, and a strip whose left-to-right order depends on a control
    /// elsewhere on the surface cannot be read from position.
    pub roster_agent_count: i64,
    /// Agents on this provider that recorded anything in the window. What the
    /// card actually prints, so every tile is denominated the same way.
    pub active_agent_count: i64,
    pub active_ms: i64,
    pub turns: i64,
    /// Billable tokens. `None` when the provider publishes no token accounting —
    /// never `Some(0)`, which would rank it the thriftiest rather than the
    /// unmeasured one.
    pub total_tokens: Option<i64>,
    pub files_touched: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub tokens_reported: bool,
    /// The trend measure per bucket, aligned to [`TelemetryFleetDto::buckets`].
    pub spark: Vec<i64>,
    /// Nothing recorded in the window. Still listed, dimmed — the same treatment
    /// idle agents get in the table below.
    pub idle: bool,
}

/// The Dashboard payload: every agent's rates over one trailing window.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryFleetDto {
    pub window: HorizonWindow,
    pub window_minutes: i64,
    pub rows: Vec<TelemetryFleetRowDto>,
    pub maxima: TelemetryFleetMaximaDto,
    pub buckets: Vec<String>,
    pub trend_measure: String,
    pub grain: String,
    /// The habitat as a whole, for the strip's leading card.
    ///
    /// A separate field rather than the first element of `providers`, so a
    /// consumer iterating providers cannot pick up the total by accident. Its
    /// distinct-count measures are queried, not summed: an agent that ran on two
    /// providers appears in both provider rows, and a file touched from two
    /// providers is one file.
    pub habitat: TelemetryFleetProviderDto,
    /// One card per provider in the roster, already in display order.
    pub providers: Vec<TelemetryFleetProviderDto>,
    /// The largest value across the provider cards, excluding the habitat.
    ///
    /// The habitat is the sum of these, so it dominates by construction; scaling
    /// the provider cards against it would flatten every one of them onto the
    /// floor. Comparable things share a scale, and a total is not comparable to
    /// its parts, so the habitat card normalizes against itself instead.
    pub provider_maxima: TelemetryFleetMaximaDto,
}

/// Narrowest window the Dashboard will read, in minutes.
///
/// Below this the rate is dominated by whether a single turn happened to land
/// inside the window, which reports noise as a signal.
const MIN_FLEET_WINDOW_MINUTES: i64 = 5;

/// Widest window, in minutes.
///
/// Generous rather than principled. Capping this at a day was defensible on the
/// theory that anything longer belongs to Analytics, and simply hid an agent
/// that worked hard earlier in the week from a surface whose job is to say what
/// the habitat has been doing.
const MAX_FLEET_WINDOW_MINUTES: i64 = 90 * 24 * 60;

/// How many columns the Dashboard sparkline may have.
const FLEET_SPARK_BUCKETS: usize = 48;

/// The measures every Dashboard figure is drawn from.
///
/// One list for the table and the strip both, so a measure added to one cannot
/// quietly go missing from the other.
const FLEET_MEASURES: [Measure; 6] = [
    Measure::ActiveMs,
    Measure::Turns,
    Measure::TotalTokens,
    Measure::Files,
    Measure::LinesAdded,
    Measure::LinesRemoved,
];

/// Every agent's rates over a trailing window.
///
/// Reads facts rather than rollups. The Dashboard's window is typically an hour,
/// and an hourly rollup cannot answer a question that fine — it would report a
/// whole hour's work as one undivided block, or miss it entirely.
///
/// See [`telemetry_overview`] for why this is `(async)`.
#[tauri::command(async)]
pub fn telemetry_fleet(
    window_minutes: Option<i64>,
    measure: Option<String>,
) -> Result<TelemetryFleetDto, String> {
    let _profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
        crate::utils::runtime_profile::RuntimeMetric::TelemetryFleetQuery,
    );
    let window_minutes = window_minutes
        .unwrap_or(60)
        .clamp(MIN_FLEET_WINDOW_MINUTES, MAX_FLEET_WINDOW_MINUTES);
    let trend_measure = match measure.as_deref() {
        None => Measure::TotalTokens,
        Some(name) => {
            Measure::parse(name).ok_or_else(|| format!("unknown telemetry measure: {name}"))?
        }
    };

    let to = Utc::now();
    let from = to - chrono::Duration::minutes(window_minutes);
    let window = HorizonWindow {
        from: from.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        to: to.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        // A trailing window is exact; nothing is floored to a bucket boundary,
        // which is precisely why this cannot be answered from the rollups.
        from_floored: false,
    };
    let labels = agent_labels();
    // Read before the connection is taken, never inside it. `get_db_conn` holds
    // a non-reentrant `std::sync::Mutex` across its closure, so a second call
    // from within one deadlocks the whole app on its only connection — not just
    // this read. `agent_labels` is hoisted for exactly the same reason.
    let roster = roster_providers();
    let hours = (window_minutes as f64) / 60.0;

    wardian_core::db::get_db_conn(|conn| {
        // Totals without a time axis. A window total cannot be derived from the
        // cells — a distinct count does not sum across buckets — but neither
        // does it need them, and building the axis is the expensive half of a
        // grid. This read used to ask `matrix_at` for six grids and discard six
        // sets of buckets: on a 1.2 GB store over 30 days that cost 1197ms
        // against 506ms for the same six answers.
        let agent_totals = grouped_totals_at(conn, &window, Dimension::Agent, &FLEET_MEASURES)?;
        let total_for = |measure: Measure, key: &str| -> i64 {
            agent_totals
                .get(&measure)
                .and_then(|totals| totals.get(key))
                .copied()
                .unwrap_or(0)
        };

        // The one grid this read does need: the trend measure's cells.
        let trend = grouped_matrix_at(
            conn,
            &window,
            Dimension::Agent,
            trend_measure,
            usize::MAX,
            Some(FLEET_SPARK_BUCKETS),
        )?;

        // Which agents can report tokens at all, so an unmeasured provider reads
        // as absent rather than as the quietest agent in the habitat.
        //
        // Read from the facts over this exact window, never from `breakdown`.
        // That reads `telemetry_rollup_hourly WHERE bucket_start >= from`, and a
        // trailing window shorter than an hour begins *inside* a bucket whose
        // start precedes it — so a 15-minute view matched no rollup row at all
        // and turned every measured token total into "unreported", hiding the
        // burn rates this surface exists to show.
        let reported = token_reporting_agents(conn, &window.from, &window.to)?;

        let bucket_count = trend.buckets.len();
        let sparks: std::collections::HashMap<String, Vec<i64>> = trend
            .rows
            .into_iter()
            .map(|row| (row.key, row.cells))
            .collect();

        let mut rows: Vec<TelemetryFleetRowDto> = labels
            .iter()
            .map(|(key, label)| {
                let turns_total = total_for(Measure::Turns, key);
                let tokens_total = total_for(Measure::TotalTokens, key);
                let active_ms = total_for(Measure::ActiveMs, key);
                let files_touched = total_for(Measure::Files, key);
                let lines_added = total_for(Measure::LinesAdded, key);
                let lines_removed = total_for(Measure::LinesRemoved, key);
                let tokens_reported = reported.contains(key);

                TelemetryFleetRowDto {
                    key: key.clone(),
                    label: label.name.clone(),
                    sublabel: label.class.clone(),
                    tokens_per_hour: tokens_reported
                        .then(|| (tokens_total as f64) / hours.max(f64::EPSILON)),
                    turns_per_hour: (turns_total as f64) / hours.max(f64::EPSILON),
                    active_ms,
                    turns: turns_total,
                    total_tokens: tokens_reported.then_some(tokens_total),
                    files_touched,
                    lines_added,
                    lines_removed,
                    tokens_reported,
                    idle: turns_total == 0
                        && active_ms == 0
                        && files_touched == 0
                        && tokens_total == 0,
                    spark: sparks
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| vec![0; bucket_count]),
                }
            })
            .collect();

        rows.sort_by(|left, right| left.label.cmp(&right.label));

        let mut maxima = TelemetryFleetMaximaDto::default();
        for row in &rows {
            maxima.tokens_per_hour = maxima
                .tokens_per_hour
                .max(row.tokens_per_hour.unwrap_or(0.0));
            maxima.turns_per_hour = maxima.turns_per_hour.max(row.turns_per_hour);
            maxima.turns = maxima.turns.max(row.turns);
            maxima.active_ms = maxima.active_ms.max(row.active_ms);
            maxima.total_tokens = maxima.total_tokens.max(row.total_tokens.unwrap_or(0));
            maxima.files_touched = maxima.files_touched.max(row.files_touched);
            maxima.lines = maxima.lines.max(row.lines_added + row.lines_removed);
            maxima.spark = maxima
                .spark
                .max(row.spark.iter().copied().max().unwrap_or(0));
        }

        // The strip above the table, built from the same grids at provider
        // granularity. It reads the same window and the same trend measure as
        // the table, because a card quoting a total the rows beneath it do not
        // add up to makes the surface wrong whichever figure is right.
        // Totals without a time axis, because the cards print figures and draw
        // exactly one shape. Asking `matrix_at` six more times would buy six
        // more sets of buckets to throw away, and on a real 1.2 GB store the
        // axis is the expensive half: 734ms against 234ms for the same six
        // answers over a trailing 30 days.
        let provider_totals = totals_at(conn, &window, Dimension::Provider, &FLEET_MEASURES)?;
        let total_of = |measure: Measure, provider: &str| -> i64 {
            provider_totals
                .get(&measure)
                .and_then(|totals| totals.get(provider))
                .copied()
                .unwrap_or(0)
        };

        // The one grid the strip does need: the trend measure's cells, on the
        // same buckets and the same measure as the table's trend column.
        let provider_trend = matrix_at(
            conn,
            &window,
            Dimension::Provider,
            trend_measure,
            usize::MAX,
            Some(FLEET_SPARK_BUCKETS),
        )?;
        let provider_sparks: std::collections::HashMap<String, Vec<i64>> = provider_trend
            .rows
            .into_iter()
            .map(|row| (row.key, row.cells))
            .collect();
        let provider_reported = token_reporting_providers(conn, &window.from, &window.to)?;
        let active_agents = active_agents_by_provider(conn, &window.from, &window.to)?;

        // Every provider the habitat knows about: named by the roster, plus any
        // that recorded work without a surviving agent. A deleted agent's turns
        // are still spend that happened.
        let mut names: std::collections::BTreeSet<String> = roster.keys().cloned().collect();
        for totals in provider_totals.values() {
            names.extend(totals.keys().cloned());
        }

        let mut providers: Vec<TelemetryFleetProviderDto> = names
            .into_iter()
            .map(|provider| {
                let turns = total_of(Measure::Turns, &provider);
                let tokens = total_of(Measure::TotalTokens, &provider);
                let active_ms = total_of(Measure::ActiveMs, &provider);
                let files_touched = total_of(Measure::Files, &provider);
                let tokens_reported = provider_reported.contains(&provider);

                TelemetryFleetProviderDto {
                    roster_agent_count: roster.get(&provider).copied().unwrap_or(0),
                    active_agent_count: active_agents
                        .get(&provider)
                        .map(|agents| agents.len() as i64)
                        .unwrap_or(0),
                    active_ms,
                    turns,
                    total_tokens: tokens_reported.then_some(tokens),
                    files_touched,
                    lines_added: total_of(Measure::LinesAdded, &provider),
                    lines_removed: total_of(Measure::LinesRemoved, &provider),
                    tokens_reported,
                    spark: provider_sparks
                        .get(&provider)
                        .cloned()
                        .unwrap_or_else(|| vec![0; bucket_count]),
                    // Presence counts, not just magnitude. `active_ms` is
                    // clamped in whole seconds, so a sub-second span rounds to
                    // zero — and a card that dims itself while its header reads
                    // "1 active" is contradicting itself on screen.
                    idle: turns == 0
                        && active_ms == 0
                        && files_touched == 0
                        && tokens == 0
                        && active_agents
                            .get(&provider)
                            .is_none_or(|agents| agents.is_empty()),
                    provider,
                }
            })
            .collect();

        order_provider_cards(&mut providers);
        let provider_maxima = provider_maxima(&providers);
        let habitat = habitat_card(
            &providers,
            HabitatCounts {
                // Agents with a provider, not every configured agent. The
                // cards exclude an agent whose provider is unrecorded, so
                // counting them here would print a habitat total in the same
                // tooltip vocabulary that its own cards cannot sum to.
                roster_agents: roster.values().sum(),
                active_agents: active_agents
                    .values()
                    .flatten()
                    .collect::<std::collections::HashSet<_>>()
                    .len() as i64,
                turns: distinct_turns(conn, &window.from, &window.to)?,
                files_touched: distinct_files(conn, &window.from, &window.to)?,
            },
            bucket_count,
        );

        Ok(TelemetryFleetDto {
            window,
            window_minutes,
            rows,
            maxima,
            buckets: trend.buckets,
            trend_measure: trend_measure.as_str().to_string(),
            grain: trend.grain.as_str().to_string(),
            habitat,
            providers,
            provider_maxima,
        })
    })
    .map_err(|error| format!("could not read telemetry fleet: {error}"))
}

/// Put the provider cards in the order the strip lays them out.
///
/// Roster frequency, descending — how often the operator reaches for each
/// provider when configuring an agent. Deliberately *not* in-window activity:
/// that order moves cards sideways whenever the window setting changes, and a
/// strip whose left-to-right order depends on a control elsewhere on the surface
/// cannot be read from position.
///
/// Silent providers sink below active ones the way idle agents do in the table.
/// They stay listed, because a provider you have configured and are not using is
/// exactly the answer to "where can I spend what is left".
fn order_provider_cards(providers: &mut [TelemetryFleetProviderDto]) {
    providers.sort_by(|left, right| {
        left.idle
            .cmp(&right.idle)
            .then(right.roster_agent_count.cmp(&left.roster_agent_count))
            .then(right.turns.cmp(&left.turns))
            .then(left.provider.cmp(&right.provider))
    });
}

/// The largest value in each measure across the provider cards.
///
/// The habitat is excluded on purpose — it is their sum, so including it would
/// peg the denominator at a value no provider can approach and flatten every
/// card onto the floor.
fn provider_maxima(providers: &[TelemetryFleetProviderDto]) -> TelemetryFleetMaximaDto {
    let mut maxima = TelemetryFleetMaximaDto::default();
    for card in providers {
        maxima.turns = maxima.turns.max(card.turns);
        maxima.active_ms = maxima.active_ms.max(card.active_ms);
        maxima.total_tokens = maxima.total_tokens.max(card.total_tokens.unwrap_or(0));
        maxima.files_touched = maxima.files_touched.max(card.files_touched);
        maxima.lines = maxima.lines.max(card.lines_added + card.lines_removed);
        maxima.spark = maxima
            .spark
            .max(card.spark.iter().copied().max().unwrap_or(0));
    }
    maxima
}

/// The habitat figures that cannot be derived from the provider cards.
///
/// Every one of these is a distinct count, which is exactly why it is passed in
/// rather than folded out of `providers`: an agent that ran on two providers
/// appears in both cards, and a file edited from two providers is one file.
pub struct HabitatCounts {
    pub roster_agents: i64,
    pub active_agents: i64,
    pub turns: i64,
    pub files_touched: i64,
}

/// Assemble the strip's leading card.
///
/// Additive measures sum across the providers; the distinct counts come from
/// `counts`, queried against the same window. A turn belongs to exactly one
/// provider so turns would in fact survive summing, but files would not, and
/// answering both the same way is what keeps the pair from drifting apart the
/// next time one of them is edited.
fn habitat_card(
    providers: &[TelemetryFleetProviderDto],
    counts: HabitatCounts,
    bucket_count: usize,
) -> TelemetryFleetProviderDto {
    let tokens_reported = providers.iter().any(|card| card.tokens_reported);
    TelemetryFleetProviderDto {
        provider: "all".to_string(),
        roster_agent_count: counts.roster_agents,
        active_agent_count: counts.active_agents,
        active_ms: providers.iter().map(|card| card.active_ms).sum(),
        turns: counts.turns,
        // Summed across only the providers that report tokens, so a habitat
        // running one measured and one unmeasured provider still shows the
        // measured spend rather than collapsing the pair to unreported.
        total_tokens: tokens_reported
            .then(|| providers.iter().filter_map(|card| card.total_tokens).sum()),
        files_touched: counts.files_touched,
        lines_added: providers.iter().map(|card| card.lines_added).sum(),
        lines_removed: providers.iter().map(|card| card.lines_removed).sum(),
        tokens_reported,
        // Per-bucket sums. For a distinct-count trend measure these overstate
        // the same way the matrix's own cells already do — the axis is there for
        // shape, and the card's printed totals are the figures to read.
        spark: (0..bucket_count)
            .map(|index| {
                providers
                    .iter()
                    .map(|card| card.spark.get(index).copied().unwrap_or(0))
                    .sum()
            })
            .collect(),
        idle: providers.iter().all(|card| card.idle),
    }
}

/// Providers that reported any token accounting inside an exact window.
///
/// The provider-level twin of [`token_reporting_agents`], and fact-backed for
/// the same reason: a trailing window routinely starts mid-hour, where a rollup
/// filtered on `bucket_start >= from` matches nothing at all.
fn token_reporting_providers(
    conn: &rusqlite::Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT provider FROM telemetry_turns
         WHERE ended_at >= ?1 AND ended_at < ?2
           AND (input_tokens IS NOT NULL
                OR output_tokens IS NOT NULL
                OR cached_input_tokens IS NOT NULL)",
    )?;
    let rows = stmt.query_map(rusqlite::params![from, to], |row| row.get::<_, String>(0))?;
    rows.collect()
}

/// Which agents were active on each provider inside an exact window.
///
/// Sets rather than counts, so the habitat's own figure can be taken across the
/// union: an agent that ran on two providers is counted once there and once in
/// each provider card, which are different and both correct.
///
/// Three fact tables, because activity is not only turns. An agent that spent
/// the window editing without a recorded turn was still working, and reading
/// only `telemetry_turns` would report it as absent. The provider is read from
/// the facts rather than from the agent's current configuration — the roster
/// holds where an agent points *now*, which is the wrong answer for a window
/// that reaches back before it was changed.
fn active_agents_by_provider(
    conn: &rusqlite::Connection,
    from: &str,
    to: &str,
) -> rusqlite::Result<std::collections::HashMap<String, std::collections::HashSet<String>>> {
    let mut stmt = conn.prepare(
        "SELECT provider, session_id FROM telemetry_turns
             WHERE ended_at >= ?1 AND ended_at < ?2
         UNION
         SELECT provider, session_id FROM telemetry_edits
             WHERE occurred_at >= ?1 AND occurred_at < ?2
         UNION
         -- Overlap, not containment, because that is how the active-time
         -- measure clips a span to the window. Filtering on `last_event_at`
         -- instead let a card report active time with nobody active on it: a
         -- span whose last event fell just before the window still reaches into
         -- it through the singleton credit.
         SELECT provider, session_id FROM telemetry_activity
             WHERE ended_at > ?1 AND started_at < ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![from, to], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut by_provider: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for row in rows {
        let (provider, session_id) = row?;
        by_provider.entry(provider).or_default().insert(session_id);
    }
    Ok(by_provider)
}

/// Turns across the whole habitat in an exact window.
///
/// Not the sum of the provider cards. A turn belongs to exactly one provider so
/// summing would in fact agree here, but files would not, and asking both the
/// same way is what stops the pair drifting apart when one is next edited.
fn distinct_turns(conn: &rusqlite::Connection, from: &str, to: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        // Turns without an id are still turns; they simply have no identity to
        // collapse on, so each event counts once.
        "SELECT COUNT(DISTINCT COALESCE(turn_id, event_key)) FROM telemetry_turns
         WHERE ended_at >= ?1 AND ended_at < ?2",
        rusqlite::params![from, to],
        |row| row.get(0),
    )
}

/// Distinct files touched across the whole habitat in an exact window.
///
/// Emphatically not the sum of the provider cards: one file edited from two
/// providers is one file, and summing reports it as two.
fn distinct_files(conn: &rusqlite::Connection, from: &str, to: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(DISTINCT path) FROM telemetry_edits
         WHERE occurred_at >= ?1 AND occurred_at < ?2",
        rusqlite::params![from, to],
        |row| row.get(0),
    )
}

/// How many configured agents name each provider.
///
/// The strip's ordering key. Best-effort like [`agent_labels`]: a failure here
/// costs the preferred order, not the data, so it degrades to alphabetical
/// rather than failing the read.
fn roster_providers() -> std::collections::HashMap<String, i64> {
    wardian_core::db::get_all_agents()
        .map(|agents| {
            let mut counts: std::collections::HashMap<String, i64> =
                std::collections::HashMap::new();
            // An agent with no recorded provider counts toward none of them. It
            // is one agent whose provider is unknown, not one more vote for
            // whichever provider happens to be the default this release.
            for provider in agents
                .into_iter()
                .filter_map(|agent| agent.provider)
                .filter(|provider| !provider.is_empty())
            {
                *counts.entry(provider).or_insert(0) += 1;
            }
            counts
        })
        .unwrap_or_default()
}

/// Read the Dashboard's saved column and window preferences.
///
/// Deliberately untyped at this boundary, and stored the same way the watchlist
/// stores its own: the frontend merges whatever is here *over* its defaults, so
/// a column added in a later release appears without a migration and a stale
/// file can never hide one. Returning `Null` on a first run is a normal answer.
#[tauri::command]
pub async fn load_dashboard_prefs() -> Result<serde_json::Value, String> {
    if let Some(home) = crate::utils::fs::get_wardian_home() {
        let path = home.join("settings/dashboard-prefs.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            return Ok(serde_json::from_str(&data).unwrap_or(serde_json::Value::Null));
        }
    }
    Ok(serde_json::Value::Null)
}

/// Persist the Dashboard preferences.
///
/// Written on every change rather than behind a save button. This is the seed a
/// *new* Dashboard instance starts from; instances already open keep their own
/// state and are never rewritten by this.
#[tauri::command]
pub async fn save_dashboard_prefs(prefs: serde_json::Value) -> Result<(), String> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| "Could not find home directory".to_string())?;
    let _ = std::fs::create_dir_all(home.join("settings"));
    let json = serde_json::to_string_pretty(&prefs).map_err(|error| error.to_string())?;
    std::fs::write(home.join("settings/dashboard-prefs.json"), json)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// The matrix Analytics is built on: rows × time, for one measure.
///
/// See [`telemetry_overview`] for why this is `(async)`.
#[tauri::command(async)]
pub fn telemetry_matrix(
    horizon: String,
    dimension: String,
    measure: String,
    limit: Option<usize>,
) -> Result<TelemetryMatrixDto, String> {
    let _profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
        crate::utils::runtime_profile::RuntimeMetric::TelemetryMatrixQuery,
    );
    let horizon =
        Horizon::parse(&horizon).ok_or_else(|| format!("unknown telemetry horizon: {horizon}"))?;
    let dimension = Dimension::parse(&dimension)
        .ok_or_else(|| format!("unknown telemetry dimension: {dimension}"))?;
    let measure =
        Measure::parse(&measure).ok_or_else(|| format!("unknown telemetry measure: {measure}"))?;
    let window = resolve_horizon(horizon, Utc::now());
    let labels = agent_labels();
    let row_limit = limit.unwrap_or(MATRIX_ROW_LIMIT).clamp(1, MATRIX_ROW_CAP);

    wardian_core::db::get_db_conn(|conn| {
        let grid = if dimension == Dimension::Agent {
            grouped_matrix_at(conn, &window, dimension, measure, row_limit, None)?
        } else {
            wardian_core::telemetry::matrix::matrix(conn, &window, dimension, measure, row_limit)?
        };
        Ok(TelemetryMatrixDto {
            dimension: dimension.as_str().to_string(),
            measure: measure.as_str().to_string(),
            grain: grid.grain.as_str().to_string(),
            rows: grid
                .rows
                .into_iter()
                .map(|row| TelemetryMatrixRowDto {
                    label: label_for(&row.key, dimension, &labels),
                    sublabel: sublabel_for(&row.key, dimension, &labels),
                    key: row.key,
                    cells: row.cells,
                    total: row.total,
                })
                .collect(),
            buckets: grid.buckets,
            max_cell: grid.max_cell,
            cells_are_not_additive: grid.cells_are_not_additive,
            window,
        })
    })
    .map_err(|error| format!("could not read telemetry matrix: {error}"))
}

/// Rows returned when a caller does not say.
const MATRIX_ROW_LIMIT: usize = 40;

/// Hard ceiling, so a caller cannot ask for an unbounded grid.
const MATRIX_ROW_CAP: usize = 200;

/// See [`telemetry_overview`] for why this is `(async)`.
#[tauri::command(async)]
pub fn telemetry_series(horizon: String, dimension: String) -> Result<Vec<SeriesPoint>, String> {
    let horizon =
        Horizon::parse(&horizon).ok_or_else(|| format!("unknown telemetry horizon: {horizon}"))?;
    let dimension = Dimension::parse(&dimension)
        .ok_or_else(|| format!("unknown telemetry dimension: {dimension}"))?;
    let window = resolve_horizon(horizon, Utc::now());

    wardian_core::db::get_db_conn(|conn| {
        if dimension == Dimension::Agent {
            Ok(grouped_series(conn, dimension, &window.from, &window.to)?)
        } else {
            Ok(series(conn, dimension, &window.from, &window.to)?)
        }
    })
    .map_err(|error| format!("could not read telemetry series: {error}"))
}

/// See [`telemetry_overview`] for why this is `(async)`.
#[tauri::command(async)]
pub fn telemetry_activity(horizon: String) -> Result<Vec<IntervalFact>, String> {
    let horizon =
        Horizon::parse(&horizon).ok_or_else(|| format!("unknown telemetry horizon: {horizon}"))?;
    let window = resolve_horizon(horizon, Utc::now());

    wardian_core::db::get_db_conn(|conn| Ok(activity_intervals(conn, &window.from, &window.to)?))
        .map_err(|error| format!("could not read telemetry activity: {error}"))
}

/// Advance every source now, rather than waiting for the scheduled pass.
///
/// Exists so a surface can offer a refresh that means something. Ingest is
/// idempotent, so an extra pass costs a cursor comparison per source.
#[tauri::command]
pub async fn telemetry_refresh(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let report = crate::state::telemetry_ingest::run_ingest_cycle(&state).await;
    Ok(serde_json::json!({
        "sources": report.sources,
        "advanced": report.advanced,
        "turns": report.turns,
        "edits": report.edits,
        "intervals": report.intervals,
        "buckets_recomputed": report.buckets_recomputed,
        "unavailable": report.unavailable,
        "failures": report.failures,
    }))
}

fn to_dtos(
    rows: Vec<BreakdownRow>,
    dimension: Dimension,
    labels: &AgentLabels,
) -> Vec<TelemetryBreakdownRowDto> {
    rows.into_iter()
        .map(|row| TelemetryBreakdownRowDto {
            label: label_for(&row.key, dimension, labels),
            processed_tokens: row.tokens.processed_total(),
            key: row.key,
            active: row.active,
            turns: row.turns,
            tokens: row.tokens,
            files_touched: row.files_touched,
            lines_added: row.lines_added,
            lines_removed: row.lines_removed,
            agent_count: row.agent_count,
            tokens_reported: row.tokens_reported,
        })
        .collect()
}

/// What to print for a key.
///
/// A model row can carry an empty key, because `model` is nullable in the rollup
/// and SQL groups nulls together. That is a real state — a turn whose model was
/// never stated — and it has to read as unknown rather than as a blank row.
fn label_for(key: &str, dimension: Dimension, labels: &AgentLabels) -> String {
    if key.is_empty() {
        return match dimension {
            Dimension::Model => "Unknown model".to_string(),
            Dimension::Provider => "Unknown provider".to_string(),
            Dimension::Agent => "Unknown agent".to_string(),
        };
    }
    match dimension {
        // A session UUID is not a name. Falling back to the key keeps a deleted
        // agent's history addressable instead of dropping the row.
        Dimension::Agent => labels
            .get(key)
            .map(|label| label.name.clone())
            .unwrap_or_else(|| key.to_string()),
        _ => key.to_string(),
    }
}

/// What a surface should print for one agent.
#[derive(Debug, Clone)]
pub struct AgentLabel {
    pub name: String,
    pub class: Option<String>,
}

type AgentLabels = std::collections::HashMap<String, AgentLabel>;

/// Session id to display name and class, for every agent the app knows about.
///
/// Best-effort: a failure here costs labels, not data, so it degrades to the
/// raw keys rather than failing the whole read.
fn agent_labels() -> AgentLabels {
    wardian_core::db::get_all_agents()
        .map(|agents| {
            agents
                .into_iter()
                .map(|agent| {
                    (
                        agent.session_id,
                        AgentLabel {
                            name: agent.session_name,
                            class: agent.agent_class,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The secondary line under a row's label.
///
/// Only agents have one worth showing. A provider or model row is already named
/// by the only attribute it has.
fn sublabel_for(key: &str, dimension: Dimension, labels: &AgentLabels) -> Option<String> {
    match dimension {
        Dimension::Agent => labels.get(key).and_then(|label| label.class.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_key_reads_as_unknown_rather_than_blank() {
        // `model` is nullable in the rollup and SQL groups nulls together, so
        // this row genuinely occurs: turns whose model was never stated.
        let labels = AgentLabels::new();
        assert_eq!(label_for("", Dimension::Model, &labels), "Unknown model");
        assert_eq!(label_for("", Dimension::Agent, &labels), "Unknown agent");
    }

    #[test]
    fn an_agent_key_resolves_to_its_name() {
        let labels = AgentLabels::from([(
            "uuid-1".to_string(),
            AgentLabel {
                name: "Scout".to_string(),
                class: Some("Coder".to_string()),
            },
        )]);
        assert_eq!(label_for("uuid-1", Dimension::Agent, &labels), "Scout");
        // The class rides along as the row's second line, so a grid of 40 rows
        // stays readable without a lookup elsewhere.
        assert_eq!(
            sublabel_for("uuid-1", Dimension::Agent, &labels).as_deref(),
            Some("Coder")
        );
        // Only agents have one; a provider or model row is already named by the
        // only attribute it has.
        assert_eq!(sublabel_for("codex", Dimension::Provider, &labels), None);
    }

    #[test]
    fn an_unknown_agent_falls_back_to_its_key() {
        // A deleted agent still has history. Dropping the row would make the
        // totals disagree with the summary they were computed from.
        let labels = AgentLabels::new();
        assert_eq!(label_for("uuid-9", Dimension::Agent, &labels), "uuid-9");
    }

    fn card(provider: &str, roster: i64, turns: i64) -> TelemetryFleetProviderDto {
        TelemetryFleetProviderDto {
            provider: provider.to_string(),
            roster_agent_count: roster,
            active_agent_count: 1,
            active_ms: 1_000,
            turns,
            total_tokens: Some(100),
            files_touched: 3,
            lines_added: 10,
            lines_removed: 4,
            tokens_reported: true,
            spark: vec![1, 2, 3],
            idle: turns == 0,
        }
    }

    fn telemetry_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        wardian_core::telemetry::run_telemetry_migrations(&conn).unwrap();
        conn
    }

    fn add_edit(conn: &rusqlite::Connection, provider: &str, session: &str, path: &str, at: &str) {
        conn.execute(
            "INSERT INTO telemetry_edits
             (event_key, session_id, provider, occurred_at, path, op, source_key, source_path)
             VALUES (?1, ?2, ?3, ?4, ?5, 'edit', 'k', 'p')",
            // Provider is part of the key because two providers editing the
            // same file are two different sources, which is how the real ingest
            // writes them.
            rusqlite::params![
                format!("{provider}-{session}-{path}-{at}"),
                session,
                provider,
                at,
                path
            ],
        )
        .unwrap();
    }

    fn add_turn(conn: &rusqlite::Connection, provider: &str, session: &str, turn: &str, at: &str) {
        conn.execute(
            "INSERT INTO telemetry_turns
             (event_key, session_id, provider, turn_id, ended_at, source_key, source_path)
             VALUES (?1, ?2, ?3, ?4, ?5, 'k', 'p')",
            rusqlite::params![
                format!("{provider}-{session}-{turn}"),
                session,
                provider,
                turn,
                at
            ],
        )
        .unwrap();
    }

    const FROM: &str = "2026-08-20T00:00:00.000Z";
    const TO: &str = "2026-08-20T01:00:00.000Z";
    const DURING: &str = "2026-08-20T00:30:00.000Z";

    /// Run `telemetry_fleet` against a real initialised store, under a timeout.
    ///
    /// The timeout is the whole point. `get_db_conn` holds a non-reentrant
    /// `std::sync::Mutex` across its closure, so any helper that takes the
    /// connection again from inside one deadlocks — and a deadlocked test hangs
    /// CI instead of failing it. This turns that into a verdict.
    fn fleet_within(timeout: std::time::Duration) -> Result<TelemetryFleetDto, String> {
        let _lock = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().expect("temp wardian home");
        let previous = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("init test database");

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(telemetry_fleet(Some(60), Some("total_tokens".into())));
        });
        let answer = receiver.recv_timeout(timeout);

        match previous {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }

        let Ok(answer) = answer else {
            // Panicking here would report this one test and then hang the rest:
            // the timed-out thread is still blocked on `DB_CONN`, a process-wide
            // `std::sync::Mutex` it will never release, so every later test that
            // touches the store waits forever. There is no way to reclaim it, so
            // the binary ends here with a verdict instead of stalling CI.
            eprintln!(
                "telemetry_fleet did not answer within {timeout:?}. The global DB mutex is                  almost certainly re-entered — check that every helper needing the store is                  read *before* `get_db_conn`, not inside its closure."
            );
            std::process::exit(101);
        };
        answer
    }

    #[test]
    fn the_fleet_read_never_takes_the_database_twice() {
        // Regression. The roster lookup that orders the provider strip was
        // called from *inside* `get_db_conn`, whose guard is held across the
        // closure. `get_all_agents` then asked for the same lock on the same
        // thread, wedging not just the Dashboard but every other reader of the
        // app's single global connection. Every helper this command needs from
        // outside the store is read before the connection is taken.
        let fleet = fleet_within(std::time::Duration::from_secs(20)).expect("fleet read");

        assert_eq!(fleet.window_minutes, 60);
        assert_eq!(fleet.habitat.provider, "all");
        // An empty store still answers with a habitat card, so the strip keeps
        // its shape rather than vanishing on a fresh install.
        assert!(fleet.habitat.idle);
        assert_eq!(fleet.habitat.spark.len(), fleet.buckets.len());
    }

    #[test]
    fn cards_are_ordered_by_how_often_the_operator_configures_each_provider() {
        // Not by in-window activity. That order moves cards sideways whenever
        // the window setting changes, and a strip whose left-to-right order
        // depends on a control elsewhere cannot be read from position — so the
        // provider on more agents leads even while doing less work right now.
        let mut providers = vec![
            card("opencode", 1, 900),
            card("codex", 6, 10),
            card("claude", 3, 40),
        ];
        order_provider_cards(&mut providers);

        let order: Vec<&str> = providers
            .iter()
            .map(|card| card.provider.as_str())
            .collect();
        assert_eq!(order, vec!["codex", "claude", "opencode"]);
    }

    #[test]
    fn a_silent_provider_sinks_below_the_active_ones_but_stays_listed() {
        // A provider you have configured and are not using is directly the
        // answer to "where can I spend what is left", which is why the table
        // below keeps idle agents too.
        let mut providers = vec![card("gemini", 9, 0), card("codex", 2, 40)];
        order_provider_cards(&mut providers);

        let order: Vec<&str> = providers
            .iter()
            .map(|card| card.provider.as_str())
            .collect();
        assert_eq!(order, vec!["codex", "gemini"]);
    }

    #[test]
    fn the_habitat_does_not_sum_the_counts_that_cannot_be_summed() {
        // One agent working across two providers is one agent, and one file
        // edited from both is one file. Summing the cards reports two of each —
        // not wrong by a little, a different quantity.
        let providers = vec![card("codex", 2, 10), card("claude", 2, 5)];
        let habitat = habitat_card(
            &providers,
            HabitatCounts {
                roster_agents: 4,
                active_agents: 1,
                turns: 15,
                files_touched: 3,
            },
            3,
        );

        assert_eq!(habitat.active_agent_count, 1);
        assert_eq!(habitat.files_touched, 3);
        // The additive ones do sum, and must.
        assert_eq!(habitat.lines_added, 20);
        assert_eq!(habitat.active_ms, 2_000);
        assert_eq!(habitat.spark, vec![2, 4, 6]);
    }

    #[test]
    fn the_habitat_reports_measured_spend_even_beside_an_unmeasured_provider() {
        // Antigravity publishes no token accounting. Letting it drag the habitat
        // card to unreported would hide spend that was in fact measured.
        let mut unmeasured = card("antigravity", 1, 4);
        unmeasured.total_tokens = None;
        unmeasured.tokens_reported = false;

        let habitat = habitat_card(
            &[card("codex", 2, 10), unmeasured],
            HabitatCounts {
                roster_agents: 3,
                active_agents: 2,
                turns: 14,
                files_touched: 6,
            },
            3,
        );

        assert_eq!(habitat.total_tokens, Some(100));
        assert!(habitat.tokens_reported);
    }

    #[test]
    fn a_habitat_of_only_unmeasured_providers_reports_no_tokens_rather_than_zero() {
        let mut unmeasured = card("antigravity", 1, 4);
        unmeasured.total_tokens = None;
        unmeasured.tokens_reported = false;

        let habitat = habitat_card(
            &[unmeasured],
            HabitatCounts {
                roster_agents: 1,
                active_agents: 1,
                turns: 4,
                files_touched: 2,
            },
            3,
        );

        assert_eq!(habitat.total_tokens, None);
        assert!(!habitat.tokens_reported);
    }

    #[test]
    fn provider_maxima_leave_the_habitat_out() {
        // The habitat is the sum of these cards. Folding it into the denominator
        // would flatten every provider sparkline onto the floor.
        let providers = vec![card("codex", 2, 10), card("claude", 2, 5)];
        let maxima = provider_maxima(&providers);
        assert_eq!(maxima.spark, 3);
        assert_eq!(maxima.turns, 10);
    }

    #[test]
    fn one_file_edited_from_two_providers_is_one_file() {
        let conn = telemetry_db();
        add_edit(&conn, "codex", "uuid-1", "src/lib.rs", DURING);
        add_edit(&conn, "claude", "uuid-1", "src/lib.rs", DURING);

        assert_eq!(distinct_files(&conn, FROM, TO).unwrap(), 1);
    }

    #[test]
    fn one_agent_on_two_providers_counts_once_for_the_habitat_and_once_each_card() {
        let conn = telemetry_db();
        add_turn(&conn, "codex", "uuid-1", "t1", DURING);
        add_turn(&conn, "claude", "uuid-1", "t2", DURING);
        add_turn(&conn, "codex", "uuid-2", "t3", DURING);

        let by_provider = active_agents_by_provider(&conn, FROM, TO).unwrap();
        assert_eq!(by_provider["codex"].len(), 2);
        assert_eq!(by_provider["claude"].len(), 1);

        let distinct: std::collections::HashSet<_> = by_provider.values().flatten().collect();
        assert_eq!(distinct.len(), 2);
    }

    #[test]
    fn an_agent_that_only_edited_still_counts_as_active() {
        // Activity is not only turns. Reading `telemetry_turns` alone would
        // report an agent that spent the window editing as absent.
        let conn = telemetry_db();
        add_edit(&conn, "opencode", "uuid-3", "src/main.rs", DURING);

        let by_provider = active_agents_by_provider(&conn, FROM, TO).unwrap();
        assert_eq!(by_provider["opencode"].len(), 1);
    }

    #[test]
    fn a_card_never_dims_itself_while_claiming_an_active_agent() {
        // `active_ms` is clamped in whole seconds, so a sub-second span rounds
        // to zero and every magnitude on the card reads as nothing — while the
        // agent that produced it is genuinely present in the window. Dimming
        // that card put "1 active" in the header of a card drawn as unused.
        let conn = telemetry_db();
        conn.execute(
            "INSERT INTO telemetry_activity
             (session_id, provider, started_at, ended_at, last_event_at, event_count, method, source_key)
             VALUES ('uuid-1', 'codex', ?1, ?1, ?1, 1, 'measured', 'k')",
            rusqlite::params![DURING],
        )
        .unwrap();

        let active = active_agents_by_provider(&conn, FROM, TO).unwrap();
        assert_eq!(active["codex"].len(), 1);

        // The predicate that decides dimming must agree with that count.
        let present = active
            .get("codex")
            .is_none_or(|agents: &std::collections::HashSet<String>| agents.is_empty());
        assert!(
            !present,
            "a provider with a recorded agent must not read as idle"
        );
    }

    #[test]
    fn work_outside_the_window_is_not_counted() {
        let conn = telemetry_db();
        add_turn(&conn, "codex", "uuid-1", "t1", "2026-08-19T23:00:00.000Z");
        add_edit(
            &conn,
            "codex",
            "uuid-1",
            "src/lib.rs",
            "2026-08-20T02:00:00.000Z",
        );

        assert!(active_agents_by_provider(&conn, FROM, TO)
            .unwrap()
            .is_empty());
        assert_eq!(distinct_turns(&conn, FROM, TO).unwrap(), 0);
        assert_eq!(distinct_files(&conn, FROM, TO).unwrap(), 0);
    }

    #[test]
    fn a_provider_reports_tokens_only_when_some_turn_carried_them() {
        let conn = telemetry_db();
        add_turn(&conn, "antigravity", "uuid-1", "t1", DURING);
        add_turn(&conn, "codex", "uuid-2", "t2", DURING);
        conn.execute(
            "UPDATE telemetry_turn_facts
             SET input_tokens = 40
             WHERE provider_ref = (SELECT string_id FROM telemetry_strings
                                   WHERE kind = 'provider' AND value = 'codex')",
            [],
        )
        .unwrap();

        let reporting = token_reporting_providers(&conn, FROM, TO).unwrap();
        assert!(reporting.contains("codex"));
        // Reported zero and reports nothing are different claims, and only the
        // second may render as unknown.
        assert!(!reporting.contains("antigravity"));
    }

    #[test]
    fn provider_and_model_keys_are_their_own_labels() {
        let labels = AgentLabels::from([(
            "codex".to_string(),
            AgentLabel {
                name: "wrong".to_string(),
                class: None,
            },
        )]);
        // The agent map must not leak into other dimensions: a provider named
        // the same as a session id would otherwise be relabelled.
        assert_eq!(label_for("codex", Dimension::Provider, &labels), "codex");
        assert_eq!(label_for("codex", Dimension::Model, &labels), "codex");
    }
}
