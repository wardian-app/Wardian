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

#[derive(Debug, Serialize, Deserialize, Clone)]
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

fn default_repeat_every() -> u32 { 1 }
fn default_end_condition() -> String { "never".to_string() }

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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowTelemetryEvent {
    pub workflow_id: String,
    pub node_id: String,
    pub status: String,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}
