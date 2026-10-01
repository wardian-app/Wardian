use crate::automation::field_type::{FieldDef, FieldType};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// Delegates to a Wardian agent.
    Agent,
    /// Deterministic step executed by the engine itself.
    Engine,
    /// Entry point that starts a run.
    Trigger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortDef {
    pub id: String,
    pub label: String,
}

impl PortDef {
    fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// The contract for one node type. Behavior lives in the engine (sub-project
/// #2), keyed by `id`; this struct is the authoring + validation contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeTypeDef {
    pub id: String,
    pub kind: NodeKind,
    pub category: String,
    pub label: String,
    pub icon: String,
    pub description: String,
    /// Whether this node has a complete runtime contract and may be authored.
    pub supported: bool,
    pub fields: Vec<FieldDef>,
    pub inputs: Vec<PortDef>,
    pub outputs: Vec<PortDef>,
    /// When set, the node's outgoing ports are derived at runtime from the
    /// named `branch_port` field instead of from `outputs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs_from_field: Option<String>,
    pub version: u32,
}

static NODE_TYPES: Lazy<Vec<NodeTypeDef>> = Lazy::new(build_registry);

/// All node types in the automation taxonomy.
pub fn node_types() -> &'static [NodeTypeDef] {
    &NODE_TYPES
}

/// Look up a node type by its `id` (e.g. `"task"`).
pub fn find_node_type(id: &str) -> Option<&'static NodeTypeDef> {
    NODE_TYPES.iter().find(|n| n.id == id)
}

fn default_in() -> Vec<PortDef> {
    vec![PortDef::new("in", "In")]
}
fn default_out() -> Vec<PortDef> {
    vec![PortDef::new("out", "Out")]
}

fn build_registry() -> Vec<NodeTypeDef> {
    vec![
        // ----- Agent steps -----
        NodeTypeDef {
            id: "task".into(),
            kind: NodeKind::Agent,
            category: "Agent".into(),
            label: "Task".into(),
            icon: "robot".into(),
            description: "Delegate work to an agent; returns structured output.".into(),
            supported: true,
            fields: vec![
                FieldDef::new("agent", FieldType::AgentRef, "Agent")
                    .required()
                    .help("Agent or role to run this task."),
                FieldDef::new("prompt", FieldType::Prompt, "Prompt").required(),
                FieldDef::new("output_schema", FieldType::JsonSchema, "Output schema"),
            ],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "decision".into(),
            kind: NodeKind::Agent,
            category: "Agent".into(),
            label: "Decision".into(),
            icon: "git-branch".into(),
            description: "Agent chooses one of the declared outgoing branches.".into(),
            supported: true,
            fields: vec![
                FieldDef::new("agent", FieldType::AgentRef, "Agent").required(),
                FieldDef::new("prompt", FieldType::Prompt, "Prompt").required(),
                FieldDef::new("choices", FieldType::BranchPort, "Choices")
                    .multiple()
                    .required()
                    .help("Unique non-empty port ids the agent may choose between; each must connect to an outgoing edge."),
            ],
            inputs: default_in(),
            outputs: vec![],
            outputs_from_field: Some("choices".into()),
            version: 1,
        },
        // ----- Engine steps -----
        NodeTypeDef {
            id: "branch".into(),
            kind: NodeKind::Engine,
            category: "Control".into(),
            label: "Branch".into(),
            icon: "git-fork".into(),
            description: "Deterministic condition on run state.".into(),
            supported: true,
            fields: vec![FieldDef::new("condition", FieldType::Text, "Condition")
                .required()
                .help(crate::automation::condition::CONDITION_HELP)],
            inputs: default_in(),
            outputs: vec![
                PortDef::new("on_true", "True"),
                PortDef::new("on_false", "False"),
            ],
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "loop".into(),
            kind: NodeKind::Engine,
            category: "Control".into(),
            label: "Loop".into(),
            icon: "repeat".into(),
            description: "Container: repeats its body subgraph until a bound is hit.".into(),
            supported: true,
            fields: vec![
                FieldDef::new("max_iterations", FieldType::Number, "Max iterations"),
                FieldDef::new("until", FieldType::Text, "Until condition")
                    .help(crate::automation::condition::CONDITION_HELP),
            ],
            inputs: default_in(),
            outputs: vec![PortDef::new("body", "Body"), PortDef::new("done", "Done")],
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "join".into(),
            kind: NodeKind::Engine,
            category: "Control".into(),
            label: "Join".into(),
            icon: "merge".into(),
            description: "Synchronization barrier; waits for all inbound edges.".into(),
            supported: true,
            fields: vec![],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "approval".into(),
            kind: NodeKind::Engine,
            category: "Control".into(),
            label: "Approval".into(),
            icon: "shield-check".into(),
            description: "Human-in-the-loop gate; parks the run until a person approves.".into(),
            supported: true,
            fields: vec![
                FieldDef::new("prompt", FieldType::Prompt, "Approval prompt")
                    .help("What the approver is signing off on."),
            ],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "shell".into(),
            kind: NodeKind::Engine,
            category: "Action".into(),
            label: "Shell".into(),
            icon: "terminal".into(),
            description: "Run a shell command.".into(),
            supported: true,
            fields: vec![
                FieldDef::new("command", FieldType::LongText, "Command").required(),
                FieldDef::new("cwd", FieldType::Path, "Working directory"),
            ],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "script".into(),
            kind: NodeKind::Engine,
            category: "Action".into(),
            label: "Script".into(),
            icon: "file-code".into(),
            description: "Run a local script through a selected runtime.".into(),
            supported: true,
            fields: vec![
                FieldDef::new(
                    "runtime",
                    FieldType::Enum {
                        options: vec!["python".into(), "node".into(), "sh".into()],
                    },
                    "Runtime",
                )
                .required(),
                FieldDef::new("path", FieldType::Path, "Script path").required(),
            ],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "state".into(),
            kind: NodeKind::Engine,
            category: "State".into(),
            label: "State".into(),
            icon: "database".into(),
            description: "Read or write storage scoped to the current automation run.".into(),
            supported: true,
            fields: vec![
                FieldDef::new(
                    "op",
                    FieldType::Enum {
                        options: vec!["get".into(), "set".into(), "merge".into(), "delete".into()],
                    },
                    "Operation",
                )
                .required(),
                FieldDef::new("entries", FieldType::KvMap, "Entries"),
            ],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "memory_commit".into(),
            kind: NodeKind::Engine,
            category: "State".into(),
            label: "Memory commit".into(),
            icon: "database".into(),
            description: "Validate and atomically commit structured agent-memory changes.".into(),
            supported: true,
            fields: vec![
                FieldDef::new("source_node", FieldType::Text, "Source node").required(),
                FieldDef::new("agent_id", FieldType::Text, "Memory agent")
                    .required()
                    .default("{{trigger.output.agent_id}}")
                    .help("Engine-owned invocation agent; only {{trigger.output.agent_id}} is accepted."),
            ],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "message_send".into(),
            kind: NodeKind::Engine,
            category: "Action".into(),
            label: "Send message".into(),
            icon: "send".into(),
            description: "Admit an exact UTF-8 workspace artifact as a host informational message. Requires the application host; no task or reply is created.".into(),
            supported: true,
            fields: vec![
                FieldDef::new("recipient", FieldType::Text, "Recipient").required()
                    .help("One exact UUID or unambiguous exact agent name. No class or broadcast selectors. Must resolve before tasks start."),
                FieldDef::new("artifact_path", FieldType::Text, "Artifact path").required()
                    .help("Workspace-relative UTF-8 file, at most 64 KiB. Use a run-specific directory such as .wardian-review/{{run.id}}/review.md; an existing artifact fails fresh-run preflight."),
            ],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "notify".into(),
            kind: NodeKind::Engine,
            category: "State".into(),
            label: "Notify".into(),
            icon: "bell".into(),
            description: "Send an operator-facing notification.".into(),
            supported: true,
            fields: vec![FieldDef::new("message", FieldType::Prompt, "Message").required()],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        NodeTypeDef {
            id: "sub_automation".into(),
            kind: NodeKind::Engine,
            category: "Action".into(),
            label: "Sub-automation".into(),
            icon: "automation".into(),
            description: "Reserved for durable child automation runs; not available for execution yet.".into(),
            supported: false,
            fields: vec![FieldDef::new("automation", FieldType::AutomationRef, "Automation").required()],
            inputs: default_in(),
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
        // ----- Triggers -----
        NodeTypeDef {
            id: "manual_trigger".into(),
            kind: NodeKind::Trigger,
            category: "Trigger".into(),
            label: "Manual Trigger".into(),
            icon: "play".into(),
            description: "Entry point. Runs on demand or when an invoker fires it.".into(),
            supported: true,
            fields: vec![FieldDef::new(
                "input_schema",
                FieldType::JsonSchema,
                "Input schema",
            )],
            inputs: vec![],
            outputs: default_out(),
            outputs_from_field: None,
            version: 1,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_the_automation_taxonomy() {
        let ids: Vec<&str> = node_types().iter().map(|n| n.id.as_str()).collect();
        for expected in [
            "task",
            "decision",
            "branch",
            "loop",
            "join",
            "shell",
            "script",
            "state",
            "notify",
            "sub_automation",
            "manual_trigger",
        ] {
            assert!(ids.contains(&expected), "missing node type: {expected}");
        }
        assert!(
            !ids.contains(&"scheduled_trigger"),
            "scheduled_trigger should be removed (scheduling is an invoker, not a node)"
        );
        assert!(
            !ids.contains(&"file_watcher"),
            "file_watcher should be removed (file-watch is an invoker, not a node)"
        );
    }

    #[test]
    fn lookup_returns_known_node_type() {
        let task = find_node_type("task").expect("task exists");
        assert_eq!(task.kind, NodeKind::Agent);
        assert!(task.fields.iter().any(|f| f.id == "prompt"));
        assert!(task.fields.iter().any(|f| f.id == "agent"));
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(find_node_type("nope").is_none());
    }

    #[test]
    fn sub_automation_is_visible_but_marked_unsupported() {
        let sub_automation = find_node_type("sub_automation").expect("sub_automation exists");
        assert!(!sub_automation.supported);
    }

    #[test]
    fn decision_derives_ports_from_a_field() {
        let decision = find_node_type("decision").expect("decision exists");
        assert_eq!(decision.kind, NodeKind::Agent);
        assert_eq!(decision.outputs_from_field.as_deref(), Some("choices"));
    }

    #[test]
    fn loop_declares_body_and_done_ports() {
        let lp = find_node_type("loop").expect("loop exists");
        let outs: Vec<&str> = lp.outputs.iter().map(|p| p.id.as_str()).collect();
        assert!(outs.contains(&"body"));
        assert!(outs.contains(&"done"));
    }

    #[test]
    fn registry_has_approval_gate() {
        let a = find_node_type("approval").expect("approval exists");
        assert_eq!(a.kind, NodeKind::Engine);
        assert_eq!(a.category, "Control");
        let outs: Vec<&str> = a.outputs.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(outs, vec!["out"]);
    }
}
