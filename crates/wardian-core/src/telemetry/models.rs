//! Data types for the habitat telemetry store.
//!
//! Facts are append-only rows derived from provider sources. Rollups are
//! derived from facts and are always recomputable, so a rollup defect is fixed
//! by recomputation rather than by migration.
//!
//! Property names are `snake_case` in both Rust and TypeScript so DTOs
//! serialize across the IPC boundary without a translation layer.

use serde::{Deserialize, Serialize};

/// How a source's ingest position is expressed.
///
/// A byte offset is meaningless for the SQLite-backed providers, so the cursor
/// is tagged rather than assumed to be a file position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorKind {
    /// Position within an append-only JSONL file.
    ByteOffset,
    /// Highest row timestamp already ingested, in epoch milliseconds.
    EpochMs,
    /// Highest event sequence number already ingested.
    EventSeq,
}

impl CursorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CursorKind::ByteOffset => "byte_offset",
            CursorKind::EpochMs => "epoch_ms",
            CursorKind::EventSeq => "event_seq",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "byte_offset" => Some(CursorKind::ByteOffset),
            "epoch_ms" => Some(CursorKind::EpochMs),
            "event_seq" => Some(CursorKind::EventSeq),
            _ => None,
        }
    }
}

/// Whether a source is a flat file or a database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Jsonl,
    Sqlite,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Jsonl => "jsonl",
            SourceKind::Sqlite => "sqlite",
        }
    }
}

/// An ingest position paired with the interpretation of its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub kind: CursorKind,
    pub value: i64,
}

impl Cursor {
    pub fn new(kind: CursorKind, value: i64) -> Self {
        Self { kind, value }
    }

    pub fn start(kind: CursorKind) -> Self {
        Self { kind, value: 0 }
    }
}

/// How an active interval's duration was established.
///
/// These are not interchangeable. `Measured` comes from a provider that reports
/// real start and end times; `Clustered` is inferred by gap-clustering event
/// timestamps and systematically overestimates, because think-time gaps shorter
/// than the threshold are counted as active. Any total that mixes them is
/// summing unlike quantities and must say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityMethod {
    Measured,
    Clustered,
    Decoded,
}

impl ActivityMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            ActivityMethod::Measured => "measured",
            ActivityMethod::Clustered => "clustered",
            ActivityMethod::Decoded => "decoded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "measured" => Some(ActivityMethod::Measured),
            "clustered" => Some(ActivityMethod::Clustered),
            "decoded" => Some(ActivityMethod::Decoded),
            _ => None,
        }
    }
}

/// A file operation recorded by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditOp {
    Add,
    Update,
    Delete,
}

impl EditOp {
    pub fn as_str(self) -> &'static str {
        match self {
            EditOp::Add => "add",
            EditOp::Update => "update",
            EditOp::Delete => "delete",
        }
    }
}

/// One completed provider turn.
///
/// Token fields are `Option` rather than defaulting to zero: a provider that
/// does not report token accounting (antigravity) must be distinguishable from
/// one that reports a genuine zero, or it renders as the cheapest provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnFact {
    /// Stable identity for this record within its source, used for
    /// deduplication.
    ///
    /// It must be derived from the record itself and never from anything that
    /// varies with where a delta happened to be cut. `turn_id` cannot serve:
    /// codex carries turn identity in a *separate* record, so an incremental
    /// read that lands between the two produces `None` where a full re-read
    /// produces `Some`. Since SQL treats NULLs as distinct, that mismatch turns
    /// a re-read into a double count rather than a no-op.
    pub event_key: String,
    pub session_id: String,
    pub provider: String,
    pub turn_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: String,
    /// **Fresh input only** — prompt tokens that were *not* served from cache.
    ///
    /// This is a normalized quantity, not whatever the provider called
    /// `input`. Providers disagree: codex reports a prompt total that includes
    /// cache reads, while opencode reports the two disjointly
    /// (`total = input + output + cache.read + cache.write`). A source must
    /// subtract if its provider is inclusive, so that summing this column means
    /// the same thing regardless of who wrote the row.
    ///
    /// Getting this wrong is close to invisible and very large: storing codex's
    /// raw figure overstated one real habitat's token usage 49-fold, because
    /// cache reads dominate and grow with conversation length. Every figure
    /// stayed plausible while being off by more than an order of magnitude.
    pub input_tokens: Option<i64>,
    /// Prompt tokens served from cache. Disjoint from [`Self::input_tokens`],
    /// so total tokens billed for input is the sum of the two.
    pub cached_input_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub context_window: Option<i64>,
    /// Populated only by opencode. Captured at ingest but not surfaced, because
    /// no other provider reports it and a partial total would mislead.
    pub cost_usd: Option<f64>,
}

/// One file changed by a provider-native edit.
///
/// Line counts are a lower bound. They cover provider-native patch application
/// only; an agent editing through a shell redirect or an interpreter one-liner
/// produces no record here at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditFact {
    /// Stable per-record identity; see [`TurnFact::event_key`].
    pub event_key: String,
    pub session_id: String,
    pub provider: String,
    pub turn_id: Option<String>,
    pub occurred_at: String,
    pub workspace: Option<String>,
    pub path: String,
    pub op: EditOp,
    pub lines_added: Option<i64>,
    pub lines_removed: Option<i64>,
}

/// A span during which the provider was working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntervalFact {
    pub session_id: String,
    pub provider: String,
    pub started_at: String,
    /// The span as displayed, including the singleton credit.
    pub ended_at: String,
    /// The last real event in this interval.
    ///
    /// Equal to `ended_at` except for a singleton, where `ended_at` carries a
    /// fixed credit so the event does not vanish. Clustering the next delta must
    /// resume from this, not from the credited end, or the gap to the following
    /// event is measured from a moment at which nothing happened.
    pub last_event_at: String,
    pub event_count: i64,
    pub method: ActivityMethod,
}

/// An account-level rate limit reading.
///
/// This is a gauge over a provider account, not a measure of an agent. Two
/// agents on one account observing 43% are observing the same 43%, so these
/// are never summed or averaged; reads take the latest observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitObservation {
    pub provider: String,
    pub limit_id: Option<String>,
    pub observed_at: String,
    pub used_percent: Option<f64>,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<String>,
    pub plan_type: Option<String>,
}

/// Parser state that outlives a single delta.
///
/// Codex states a turn's identity and model once, in a `turn_context` record,
/// and every later record inherits it. Reading incrementally cuts that stream at
/// arbitrary points, so without carrying this forward the attribution a record
/// gets would depend on when the ingest cycle happened to run — the same log
/// yielding a model on a full re-read and `None` when read in pieces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCarry {
    pub turn_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub cwd: Option<String>,
}

impl SourceCarry {
    pub fn is_empty(&self) -> bool {
        self.turn_id.is_none()
            && self.model.is_none()
            && self.effort.is_none()
            && self.cwd.is_none()
    }
}

/// Everything a source extracted from one cursor advance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedFacts {
    pub turns: Vec<TurnFact>,
    pub edits: Vec<EditFact>,
    /// Real spans reported by the provider. A source emits these *or*
    /// `event_times`, never both: clustering exists for providers that cannot
    /// report durations, so supplying both would double count.
    pub intervals: Vec<IntervalFact>,
    /// Bare event timestamps (RFC 3339) fed to the clusterer.
    pub event_times: Vec<String>,
    pub limits: Vec<LimitObservation>,
    /// Parser state as of the end of this delta, persisted so the next delta
    /// resumes with the same context a full re-read would have had.
    pub carry: SourceCarry,
}

impl ParsedFacts {
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
            && self.edits.is_empty()
            && self.intervals.is_empty()
            && self.event_times.is_empty()
            && self.limits.is_empty()
    }
}

/// Token counts, each independently nullable.
///
/// A component is `None` when no contributing turn reported it. Coalescing to
/// zero here would be the same lie the `Option` fields on [`TurnFact`] exist to
/// prevent, only harder to notice: a provider that reports input but not
/// reasoning would appear to have done no reasoning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
}

impl TokenCounts {
    /// Whether any component was reported at all.
    pub fn any_reported(&self) -> bool {
        self.input_tokens.is_some()
            || self.cached_input_tokens.is_some()
            || self.cache_write_tokens.is_some()
            || self.output_tokens.is_some()
            || self.reasoning_tokens.is_some()
    }

    /// New content processed: fresh input, cache writes, and output.
    ///
    /// **Cache reads are excluded; cache writes are not.** The two are not
    /// interchangeable. A cache read is content the model already processed
    /// once, replayed at a tenth of the price. A cache write is content it read
    /// for the *first* time, billed at or above the fresh-input rate — it is
    /// work, and omitting it is not a conservative choice.
    ///
    /// That distinction is worth more than it looks on claude, which sends
    /// almost nothing as plain input. Over a real 400-turn session: 8,446 fresh
    /// input, 1,885,871 cache writes, 462,954 output. Counting only input and
    /// output reported 471,400 against 2,357,271 actually processed — a 5x
    /// understatement that hid 99.6% of the session's new prompt content.
    ///
    /// Excluding cache reads is only meaningful because
    /// [`TurnFact::input_tokens`] is normalized to be cache-exclusive at
    /// ingest. On the same habitat cache reads ran ~50x fresh input, so a
    /// version of this that summed a provider's raw prompt total would report
    /// 3.6B tokens where 73M were processed.
    ///
    /// The name is deliberate. This was `billable_total`, which was false in
    /// both directions: cache reads are billed too, at 10%, and cache writes
    /// were billed and not counted. `None` when no component was reported.
    pub fn processed_total(&self) -> Option<i64> {
        match (
            self.input_tokens,
            self.cache_write_tokens,
            self.output_tokens,
        ) {
            (None, None, None) => None,
            (input, write, output) => {
                Some(input.unwrap_or(0) + write.unwrap_or(0) + output.unwrap_or(0))
            }
        }
    }

    pub fn add(&mut self, other: &TokenCounts) {
        fn merge(target: &mut Option<i64>, value: Option<i64>) {
            if let Some(value) = value {
                *target = Some(target.unwrap_or(0) + value);
            }
        }
        merge(&mut self.input_tokens, other.input_tokens);
        merge(&mut self.cached_input_tokens, other.cached_input_tokens);
        merge(&mut self.cache_write_tokens, other.cache_write_tokens);
        merge(&mut self.output_tokens, other.output_tokens);
        merge(&mut self.reasoning_tokens, other.reasoning_tokens);
    }
}

/// Active milliseconds, kept split by how they were established.
///
/// There is deliberately no blended total. `measured` is a duration a provider
/// reported; `clustered` is inferred from gaps between events and systematically
/// runs high. A caller that wants one number has to decide what to call it, and
/// making that decision here would bury an estimate inside something the UI
/// would present as a measurement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTime {
    pub measured_ms: i64,
    pub clustered_ms: i64,
}

impl ActiveTime {
    pub fn add(&mut self, other: &ActiveTime) {
        self.measured_ms += other.measured_ms;
        self.clustered_ms += other.clustered_ms;
    }

    /// True when both methods contributed, so any single figure the UI shows is
    /// a mixture and has to be labelled as one.
    pub fn is_mixed(&self) -> bool {
        self.measured_ms > 0 && self.clustered_ms > 0
    }
}

/// Aggregate measures over a horizon, as returned to the UI.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TelemetrySummary {
    pub active: ActiveTime,
    pub turns: i64,
    pub tokens: TokenCounts,
    pub files_touched: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub agent_count: i64,
}

/// One ranked row of a breakdown (by provider, agent, model, and so on).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BreakdownRow {
    pub key: String,
    pub active: ActiveTime,
    pub turns: i64,
    pub tokens: TokenCounts,
    pub files_touched: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub agent_count: i64,
    /// False when no contributing provider reported token accounting, so the UI
    /// can mark the row incomplete instead of implying a zero.
    pub tokens_reported: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_kind_round_trips() {
        for kind in [
            CursorKind::ByteOffset,
            CursorKind::EpochMs,
            CursorKind::EventSeq,
        ] {
            assert_eq!(CursorKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(CursorKind::parse("nonsense"), None);
    }

    #[test]
    fn activity_method_round_trips() {
        for method in [
            ActivityMethod::Measured,
            ActivityMethod::Clustered,
            ActivityMethod::Decoded,
        ] {
            assert_eq!(ActivityMethod::parse(method.as_str()), Some(method));
        }
        assert_eq!(ActivityMethod::parse("estimated"), None);
    }

    #[test]
    fn turn_fact_serializes_snake_case() {
        let turn = TurnFact {
            event_key: "k1".into(),
            session_id: "abc".into(),
            provider: "codex".into(),
            turn_id: Some("t1".into()),
            model: Some("gpt-5.6-terra".into()),
            effort: Some("high".into()),
            started_at: None,
            ended_at: "2026-08-13T18:42:49.628Z".into(),
            input_tokens: Some(92854),
            cached_input_tokens: Some(91904),
            cache_write_tokens: Some(0),
            output_tokens: Some(71),
            reasoning_tokens: Some(53),
            context_window: Some(258400),
            cost_usd: None,
        };
        let json = serde_json::to_string(&turn).unwrap();
        assert!(json.contains("\"session_id\":\"abc\""));
        assert!(json.contains("\"cached_input_tokens\":91904"));
    }

    #[test]
    fn absent_tokens_are_null_not_zero() {
        // Antigravity reports no token accounting at all. Serializing that as 0
        // would rank it as the cheapest provider rather than the unmeasured one.
        let turn = TurnFact {
            event_key: "k1".into(),
            session_id: "abc".into(),
            provider: "antigravity".into(),
            turn_id: None,
            model: None,
            effort: None,
            started_at: None,
            ended_at: "2026-08-13T18:42:49.628Z".into(),
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            context_window: None,
            cost_usd: None,
        };
        let json = serde_json::to_string(&turn).unwrap();
        assert!(json.contains("\"input_tokens\":null"));
        assert!(!json.contains("\"input_tokens\":0"));
    }

    #[test]
    fn parsed_facts_empty_by_default() {
        assert!(ParsedFacts::default().is_empty());
        assert!(SourceCarry::default().is_empty());
    }

    #[test]
    fn adding_tokens_keeps_unreported_components_null() {
        // A provider reporting input but not reasoning must not acquire a
        // reasoning total of zero by being added to one that does report it.
        let mut total = TokenCounts {
            input_tokens: Some(10),
            ..TokenCounts::default()
        };
        total.add(&TokenCounts {
            input_tokens: Some(5),
            output_tokens: Some(2),
            ..TokenCounts::default()
        });
        assert_eq!(total.input_tokens, Some(15));
        assert_eq!(total.output_tokens, Some(2));
        assert_eq!(total.reasoning_tokens, None);
    }

    #[test]
    fn a_provider_reporting_nothing_stays_unreported() {
        let mut total = TokenCounts::default();
        total.add(&TokenCounts::default());
        assert!(!total.any_reported());
        assert_eq!(total.processed_total(), None);
    }

    #[test]
    fn processed_total_excludes_cache_reads() {
        // Cache reads ran ~8-10x fresh input on real sessions, so including
        // them would overstate usage by roughly an order of magnitude.
        let tokens = TokenCounts {
            input_tokens: Some(100_544),
            cached_input_tokens: Some(730_880),
            output_tokens: Some(5_254),
            ..TokenCounts::default()
        };
        assert_eq!(tokens.processed_total(), Some(105_798));
    }

    #[test]
    fn processed_total_includes_cache_writes() {
        // The real 400-turn claude session this was measured against. Claude
        // sends almost nothing as plain input, so counting only input and
        // output reports 471,400 for work that processed 2,357,271 tokens.
        let tokens = TokenCounts {
            input_tokens: Some(8_446),
            cached_input_tokens: Some(80_194_623),
            cache_write_tokens: Some(1_885_871),
            output_tokens: Some(462_954),
            reasoning_tokens: None,
        };
        assert_eq!(tokens.processed_total(), Some(2_357_271));
        // And still nowhere near the raw sum of every component, which is the
        // figure a cache-inclusive total would have produced.
        assert_ne!(tokens.processed_total(), Some(82_551_894));
    }

    #[test]
    fn a_provider_reporting_only_cache_writes_is_not_unreported() {
        // Claude-shaped traffic where the whole prompt went into the cache. A
        // total that only consulted input and output would call this `None` and
        // render a working agent as publishing no accounting at all.
        let tokens = TokenCounts {
            cache_write_tokens: Some(4_096),
            ..TokenCounts::default()
        };
        assert_eq!(tokens.processed_total(), Some(4_096));
    }

    #[test]
    fn active_time_reports_when_it_is_a_mixture() {
        let mut active = ActiveTime {
            measured_ms: 1_000,
            clustered_ms: 0,
        };
        assert!(!active.is_mixed());
        active.add(&ActiveTime {
            measured_ms: 0,
            clustered_ms: 500,
        });
        assert!(active.is_mixed());
        assert_eq!(active.measured_ms, 1_000);
        assert_eq!(active.clustered_ms, 500);
    }
}
