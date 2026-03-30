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
    /// "one_time" | "minutes" | "hours" | "daily" | "weekly"
    pub schedule_type: String,
    /// ISO8601 for one_time, interval number for minutes/hours, HH:MM for daily, "Mon,Wed@09:00" for weekly
    pub value: String,
    pub active: bool,
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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowTelemetryEvent {
    pub workflow_id: String,
    pub node_id: String,
    pub status: String,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}
