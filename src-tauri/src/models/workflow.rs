use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowNode {
    pub id: String,
    pub r#type: String,
    pub name: Option<String>,
    pub config: serde_json::Value,
    pub depends_on: Option<Vec<String>>,
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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowTelemetryEvent {
    pub workflow_id: String,
    pub node_id: String,
    pub status: String,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}
