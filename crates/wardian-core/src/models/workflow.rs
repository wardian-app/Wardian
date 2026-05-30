use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodePosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeDependency {
    pub node_id: String,
    pub port: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowNode {
    pub id: String,
    pub r#type: String,
    pub name: Option<String>,
    pub config: serde_json::Value,
    pub parameter_schema: Option<serde_json::Value>,
    pub dependencies: Option<Vec<NodeDependency>>,
    pub position: Option<NodePosition>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowSettings {
    pub max_iterations: u32,
    pub on_limit_reached: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowDefinition {
    pub id: String,
    pub name: String,
    pub settings: WorkflowSettings,
    pub nodes: Vec<WorkflowNode>,
    /// Maps template role names to live agent session IDs.
    /// Example: {"primary_coder": "abc-123-session-id"}
    #[serde(default)]
    pub role_mappings: HashMap<String, String>,
}

#[derive(Default, Debug, Serialize, Deserialize, Clone)]
pub struct ScheduleDefinition {
    /// "interval" | "daily" | "weekly" | "monthly" | "specific_dates" | "one_time"
    pub schedule_type: String,
    /// For interval: number of minutes between runs
    #[serde(default)]
    pub interval_minutes: Option<u32>,
    /// HH:MM in local time (used by daily, weekly, monthly, specific_dates)
    #[serde(default)]
    pub time_of_day: Option<String>,
    /// For weekly: which days (e.g. ["Mon","Tue","Fri"])
    #[serde(default)]
    pub days_of_week: Option<Vec<String>>,
    /// For weekly: repeat every N weeks (default 1)
    #[serde(default = "default_repeat_every")]
    pub repeat_every: u32,
    /// For monthly: which day(s) of the month (e.g. [1, 15])
    #[serde(default)]
    pub days_of_month: Option<Vec<u32>>,
    /// For specific_dates: list of ISO date strings ["2026-05-01", "2026-06-15"]
    #[serde(default)]
    pub specific_dates: Option<Vec<String>>,
    /// ISO8601 datetime for one_time schedules
    #[serde(default)]
    pub run_at: Option<String>,
    /// End condition: "never" | "on_date" | "after_occurrences"
    #[serde(default = "default_end_condition")]
    pub end_condition: String,
    /// ISO date (YYYY-MM-DD) for end_condition = "on_date"
    #[serde(default)]
    pub end_date: Option<String>,
    /// Count for end_condition = "after_occurrences"
    #[serde(default)]
    pub max_occurrences: Option<u32>,
    /// How many times this schedule has fired (for occurrence tracking)
    #[serde(default)]
    pub occurrence_count: u32,
    pub active: bool,
}

fn default_repeat_every() -> u32 {
    1
}
fn default_end_condition() -> String {
    "never".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScheduledRun {
    pub id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub schedule: ScheduleDefinition,
    pub role_mappings: HashMap<String, String>,
    /// Human-readable description (e.g. "Every 5m", "Daily at 09:00")
    #[serde(default)]
    pub description: String,
    /// Epoch ms of next scheduled execution (computed by scheduler)
    pub next_run_epoch_ms: Option<u64>,
    /// Remaining delay when paused, in ms. Used to resume without resetting the timer.
    #[serde(default)]
    pub paused_remaining_ms: Option<u64>,
    pub is_paused: bool,
    #[serde(default)]
    pub last_run_status: Option<String>,
    #[serde(default)]
    pub last_run_error: Option<String>,
    #[serde(default)]
    pub last_run_completed_epoch_ms: Option<u64>,
}

/// A persisted v2 invoker: a blueprint + invocation context (input/bindings/provider)
/// that fires on a `ScheduleDefinition` cadence. The v2 analog of `ScheduledRun`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowSchedule {
    pub id: String,
    /// Resolves to `<home>/library/workflows/<blueprint_id>.md`.
    pub blueprint_id: String,
    pub name: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    /// Entry input params (6a `input_schema` values), passed as the run trigger.
    #[serde(default)]
    pub input: serde_json::Value,
    /// role/class -> target provider (6a bindings).
    #[serde(default)]
    pub bindings: std::collections::HashMap<String, String>,
    pub schedule: ScheduleDefinition,
    #[serde(default)]
    pub next_run_epoch_ms: Option<u64>,
    #[serde(default)]
    pub paused_remaining_ms: Option<u64>,
    #[serde(default)]
    pub is_paused: bool,
    #[serde(default)]
    pub last_run_status: Option<String>,
    #[serde(default)]
    pub last_run_error: Option<String>,
    #[serde(default)]
    pub last_run_epoch_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowTelemetryEvent {
    pub workflow_id: String,
    pub node_id: String,
    pub status: String,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[cfg(test)]
mod schedule_dto_tests {
    use super::*;

    #[test]
    fn workflow_schedule_round_trips_with_defaults() {
        let json = r#"{
            "id": "s1",
            "blueprint_id": "heartbeat",
            "name": "Heartbeat",
            "schedule": { "schedule_type": "interval", "interval_minutes": 60, "active": true }
        }"#;
        let s: WorkflowSchedule = serde_json::from_str(json).unwrap();
        assert_eq!(s.blueprint_id, "heartbeat");
        assert!(s.provider.is_none());
        assert!(s.input.is_null() || s.input.is_object());
        assert!(s.bindings.is_empty());
        assert!(!s.is_paused);
        let back = serde_json::to_string(&s).unwrap();
        let s2: WorkflowSchedule = serde_json::from_str(&back).unwrap();
        assert_eq!(s2.id, "s1");
    }
}
