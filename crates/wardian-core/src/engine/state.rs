use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    AwaitingApproval,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

/// Immutable preflight resolution, owned by the run event log, not model output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDeliveryBinding {
    pub recipient_id: String,
    pub artifact_path: String,
}

/// Full, serializable run state. `apply` is the only thing that mutates it in
/// the pure core; the driver persists it as `state.json` after each event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub run_id: String,
    pub blueprint_id: String,
    /// Hash of the parsed blueprint that created this run, when available.
    /// Legacy checkpoints leave this unset and remain replay-compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blueprint_hash: Option<String>,
    pub status: RunStatus,
    /// Node id -> status. Absent = Pending (never reached yet).
    pub nodes: BTreeMap<String, NodeStatus>,
    /// `{ "nodes": { id: { "output": .. , "prev": .. } }, "trigger": {"output": ..}, "storage": {} }`.
    pub registry: serde_json::Value,
    /// Loop node id -> current 0-based iteration.
    pub loop_iter: BTreeMap<String, u32>,
    /// Inbound edges delivered (satisfied) to a node, by edge index.
    pub delivered: BTreeMap<String, BTreeSet<usize>>,
    /// Inbound edges marked not-taken (skipped), by edge index.
    pub skipped_edges: BTreeSet<usize>,
    pub next_seq: u64,
    pub failure: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub message_deliveries: BTreeMap<String, MessageDeliveryBinding>,
}

impl RunState {
    pub fn new(run_id: impl Into<String>, blueprint_id: impl Into<String>) -> Self {
        let run_id = run_id.into();
        Self {
            run_id: run_id.clone(),
            blueprint_id: blueprint_id.into(),
            blueprint_hash: None,
            status: RunStatus::Running,
            nodes: BTreeMap::new(),
            registry: serde_json::json!({ "nodes": {}, "trigger": { "output": {} }, "storage": {}, "run": {"id": run_id} }),
            loop_iter: BTreeMap::new(),
            delivered: BTreeMap::new(),
            skipped_edges: BTreeSet::new(),
            next_seq: 0,
            failure: None,
            message_deliveries: BTreeMap::new(),
        }
    }

    /// Add fields introduced after the first checkpoint format without
    /// changing the meaning of an existing durable run.
    pub fn normalize_legacy(&mut self) {
        if let Some(registry) = self.registry.as_object_mut() {
            registry.insert("run".into(), serde_json::json!({"id": self.run_id}));
            registry
                .entry("storage")
                .or_insert_with(|| serde_json::json!({}));
        }
    }

    pub fn node_status(&self, id: &str) -> Option<NodeStatus> {
        self.nodes.get(id).copied()
    }

    pub fn status_or_pending(&self, id: &str) -> NodeStatus {
        self.node_status(id).unwrap_or(NodeStatus::Pending)
    }

    pub fn set_node_status(&mut self, id: &str, status: NodeStatus) {
        self.nodes.insert(id.to_string(), status);
    }

    pub fn set_node_output(&mut self, id: &str, output: serde_json::Value) {
        self.registry["nodes"][id]["output"] = output;
    }

    pub fn node_output(&self, id: &str) -> Option<&serde_json::Value> {
        self.registry.get("nodes")?.get(id)?.get("output")
    }

    pub fn set_trigger(&mut self, output: serde_json::Value) {
        self.registry["trigger"]["output"] = output;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_run_starts_running_with_empty_registry() {
        let s = RunState::new("run-1", "wf");
        assert_eq!(s.status, RunStatus::Running);
        assert_eq!(s.run_id, "run-1");
        assert_eq!(s.next_seq, 0);
        assert!(s.node_status("plan").is_none());
    }

    #[test]
    fn registry_set_and_read_node_output() {
        let mut s = RunState::new("r", "wf");
        s.set_node_output("plan", serde_json::json!({"k": 5}));
        assert_eq!(s.node_output("plan").unwrap()["k"], 5);
        s.set_trigger(serde_json::json!({"t": 1}));
        assert_eq!(s.registry["trigger"]["output"]["t"], 1);
    }

    #[test]
    fn run_state_round_trips() {
        let mut s = RunState::new("r", "wf");
        s.set_node_status("plan", NodeStatus::Completed);
        let j = serde_json::to_string(&s).unwrap();
        let back: RunState = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }
}
