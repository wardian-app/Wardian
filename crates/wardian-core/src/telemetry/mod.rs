//! Habitat telemetry: a rollup store over provider-native sources.
//!
//! Wardian's live [`AgentTelemetry`](crate::models::AgentTelemetry) is
//! instantaneous — recomputed each pass and discarded — so nothing historical
//! survives. This module adds the missing half: provider sources are ingested
//! once, normalized into append-only facts, and folded into hourly rollups that
//! surfaces query instead of re-reading logs.
//!
//! Three properties are load-bearing and easy to lose in maintenance:
//!
//! - **Token deltas, never cumulative gauges.** Providers report both. Summing
//!   the cumulative figure multiplies the true total by the number of calls.
//! - **Cache reads stay their own series.** Measured on real data, cache reads
//!   ran roughly ten times fresh input; folding them together makes every
//!   intensity reading meaningless.
//! - **Measured and clustered durations are different quantities.** OpenCode
//!   reports real turn times; the rest are inferred by gap clustering, which
//!   overestimates. Totals that mix them must say so.
//!
//! Coverage is uneven by nature. Antigravity publishes no token accounting at
//! all — corroborated by ccusage, which lists it unsupported for that reason —
//! and gemini is not supported. Absent measures are `NULL`, never `0`, so a
//! provider that cannot report never ranks as the cheapest one.

pub mod activity;
pub mod attribution;
pub mod horizon;
pub mod identity;
pub mod ingest;
pub mod maintenance;
pub mod matrix;
pub mod models;
pub mod query;
pub mod recovery;
pub mod rollup;
pub mod schema;
pub mod sources;
pub mod store;

pub use activity::{ACTIVE_GAP_THRESHOLD_MS, ACTIVE_SINGLETON_MS};
pub use horizon::{resolve_horizon, Horizon, HorizonWindow};
pub use identity::{canonical_path, content_key, file_fingerprint, source_key};
pub use ingest::{ingest_source, IngestError, IngestOutcome};
pub use maintenance::{
    has_expired_raw_telemetry, maintain, retention_backup_is_pending, retention_backup_is_prepared,
    retention_cutoff, retention_is_prepared, verify_backup, MaintenanceReport,
};
pub use matrix::{matrix, totals_at, Grain, Matrix, MatrixRow, Measure};
pub use models::{
    ActiveTime, ActivityMethod, BreakdownRow, Cursor, CursorKind, EditFact, EditOp, IntervalFact,
    LimitObservation, ParsedFacts, RollupRow, SourceCarry, SourceKind, TelemetrySummary,
    TokenCounts, TurnFact,
};
pub use recovery::{
    inspect_attribution, repair_attribution, AttributionRepairReport, AttributionStatus,
};
pub use schema::{run_telemetry_migrations, TELEMETRY_SCHEMA_VERSION};
pub use sources::{is_supported, source_for, SourceContext, SourceError, TelemetrySource};
