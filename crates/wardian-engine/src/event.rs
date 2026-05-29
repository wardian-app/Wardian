use serde::{Deserialize, Serialize};

/// One durable run event. The append-only `events.jsonl` log of these is the
/// source of truth; folding them via `core::apply` reconstructs `RunState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub ts: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    /// Construct with the current UTC timestamp.
    pub fn new(seq: u64, kind: EventKind) -> Self {
        Self {
            seq,
            ts: chrono::Utc::now().to_rfc3339(),
            kind,
        }
    }
    /// Construct with an explicit timestamp (used when replaying/testing).
    pub fn at(seq: u64, ts: String, kind: EventKind) -> Self {
        Self { seq, ts, kind }
    }
}

/// The discriminated event payload. `node` names the relevant node where it applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    RunStarted {
        blueprint_id: String,
        schema: u32,
        trigger: serde_json::Value,
    },
    NodeStarted {
        node: String,
    },
    NodeCompleted {
        node: String,
        output: serde_json::Value,
    },
    NodeFailed {
        node: String,
        error: String,
    },
    BranchTaken {
        node: String,
        port: String,
    },
    DecisionMade {
        node: String,
        port: String,
    },
    LoopIteration {
        node: String,
        iteration: u32,
    },
    NodeSkipped {
        node: String,
    },
    AwaitingApproval {
        node: String,
    },
    ApprovalGranted {
        node: String,
        actor: String,
        note: Option<String>,
    },
    ApprovalRejected {
        node: String,
        actor: String,
        note: Option<String>,
    },
    RunCompleted,
    RunFailed {
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serializes_with_kind_tag_and_seq() {
        let ev = Event::new(
            7,
            EventKind::NodeCompleted {
                node: "plan".into(),
                output: serde_json::json!({"decision": "go"}),
            },
        );
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["seq"], 7);
        assert_eq!(v["kind"], "node_completed");
        assert_eq!(v["node"], "plan");
        assert_eq!(v["output"]["decision"], "go");
        assert!(v["ts"].as_str().unwrap().len() >= 20); // rfc3339
    }

    #[test]
    fn event_round_trips_through_jsonl_line() {
        let ev = Event::new(
            1,
            EventKind::RunStarted {
                blueprint_id: "wf".into(),
                schema: 2,
                trigger: serde_json::json!({"x":1}),
            },
        );
        let line = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&line).unwrap();
        assert_eq!(ev, back);
    }
}
