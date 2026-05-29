use crate::blueprint::Blueprint;
use crate::field_type::FieldType;
use crate::registry::find_node_type;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// One validation finding. `code` is stable and machine-readable; `message` is
/// for humans. `node` names the offending node when applicable.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

impl Diagnostic {
    fn error(code: &'static str, message: impl Into<String>, node: Option<&str>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            node: node.map(str::to_string),
        }
    }
}

/// The result of validating a blueprint.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ValidationReport {
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
    pub fn errors(&self) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect()
    }
}

/// Validate a blueprint against the registry and the structural rules
/// (DAG-only, declared ports, container parents). Returns every finding so the
/// builder can surface them all; the engine refuses to run when `is_valid()` is
/// false.
pub fn validate(blueprint: &Blueprint) -> ValidationReport {
    let mut report = ValidationReport::default();
    let node_ids: HashSet<&str> = blueprint.nodes.iter().map(|n| n.id.as_str()).collect();

    // Duplicate ids.
    let mut seen: HashSet<&str> = HashSet::new();
    for node in &blueprint.nodes {
        if !seen.insert(node.id.as_str()) {
            report.diagnostics.push(Diagnostic::error(
                "duplicate_node_id",
                format!("duplicate node id `{}`", node.id),
                Some(&node.id),
            ));
        }
    }

    // Per-node: known type + required fields + field-value kind.
    for node in &blueprint.nodes {
        let Some(def) = find_node_type(&node.r#type) else {
            report.diagnostics.push(Diagnostic::error(
                "unknown_node_type",
                format!(
                    "unknown node type `{}` (see `wardian workflow node-types`)",
                    node.r#type
                ),
                Some(&node.id),
            ));
            continue;
        };

        for field in &def.fields {
            let present = node.fields.get(&field.id);
            if field.required && present.is_none() {
                report.diagnostics.push(Diagnostic::error(
                    "missing_required_field",
                    format!(
                        "node `{}` is missing required field `{}`",
                        node.id, field.id
                    ),
                    Some(&node.id),
                ));
            }
            if let Some(value) = present {
                if let Some(msg) = check_value_kind(&field.field_type, value) {
                    report.diagnostics.push(Diagnostic::error(
                        "invalid_field_value",
                        format!("node `{}` field `{}`: {}", node.id, field.id, msg),
                        Some(&node.id),
                    ));
                }
            }
        }

        // Container parents must point at a loop node.
        if let Some(parent_id) = &node.parent {
            let parent_is_loop = blueprint
                .find_node(parent_id)
                .map(|p| p.r#type == "loop")
                .unwrap_or(false);
            if !parent_is_loop {
                report.diagnostics.push(Diagnostic::error(
                    "invalid_parent",
                    format!(
                        "node `{}` parent `{}` is not a loop node",
                        node.id, parent_id
                    ),
                    Some(&node.id),
                ));
            }
        }
    }

    // Edges reference existing nodes.
    for edge in &blueprint.edges {
        if !node_ids.contains(edge.from.as_str()) || !node_ids.contains(edge.to.as_str()) {
            report.diagnostics.push(Diagnostic::error(
                "dangling_edge",
                format!(
                    "edge `{}` -> `{}` references a missing node",
                    edge.from, edge.to
                ),
                None,
            ));
        }
    }

    // The top-level graph must be a DAG (loops are containers, not back-edges).
    if has_cycle(blueprint) {
        report.diagnostics.push(Diagnostic::error(
            "cycle_detected",
            "graph contains a cycle; use a loop container instead of a back-edge",
            None,
        ));
    }

    report
}

/// Returns a human message when `value` cannot be the given field type.
/// Only coarse kind checks live here; deep semantic checks (e.g. a valid cron)
/// belong to later sub-projects.
fn check_value_kind(field_type: &FieldType, value: &serde_json::Value) -> Option<String> {
    match field_type {
        FieldType::Bool => value
            .is_boolean()
            .then_some(())
            .map_or(Some("expected a boolean".into()), |_| None),
        FieldType::Number => value
            .is_number()
            .then_some(())
            .map_or(Some("expected a number".into()), |_| None),
        FieldType::KvMap => value
            .is_object()
            .then_some(())
            .map_or(Some("expected an object".into()), |_| None),
        FieldType::Enum { options } => match value.as_str() {
            Some(s) if options.iter().any(|o| o == s) => None,
            Some(s) => Some(format!("`{s}` is not one of {options:?}")),
            None => Some("expected a string".into()),
        },
        // Text-like and ref-like primitives just require a string.
        _ => match value {
            serde_json::Value::String(_) => None,
            _ => Some("expected a string".into()),
        },
    }
}

/// Kahn's algorithm over only the edges between *real* nodes.
fn has_cycle(blueprint: &Blueprint) -> bool {
    let mut indegree: HashMap<&str, usize> =
        blueprint.nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &blueprint.edges {
        if indegree.contains_key(edge.from.as_str()) && indegree.contains_key(edge.to.as_str()) {
            adj.entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
            *indegree.get_mut(edge.to.as_str()).unwrap() += 1;
        }
    }
    let mut queue: Vec<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut visited = 0usize;
    while let Some(n) = queue.pop() {
        visited += 1;
        if let Some(children) = adj.get(n) {
            for &c in children {
                let d = indegree.get_mut(c).unwrap();
                *d -= 1;
                if *d == 0 {
                    queue.push(c);
                }
            }
        }
    }
    visited != blueprint.nodes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{Blueprint, Edge, Node};

    fn task(id: &str) -> Node {
        let mut fields = serde_json::Map::new();
        fields.insert("agent".into(), serde_json::json!("role:coder"));
        fields.insert("prompt".into(), serde_json::json!("do it"));
        Node {
            id: id.into(),
            r#type: "task".into(),
            name: None,
            parent: None,
            fields,
            position: None,
        }
    }

    fn base(nodes: Vec<Node>, edges: Vec<Edge>) -> Blueprint {
        Blueprint {
            schema: 2,
            id: "demo".into(),
            name: "Demo".into(),
            nodes,
            edges,
            body: String::new(),
        }
    }

    #[test]
    fn valid_blueprint_has_no_errors() {
        let bp = base(
            vec![
                Node {
                    id: "t".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                task("plan"),
            ],
            vec![Edge {
                from: "t".into(),
                to: "plan".into(),
                from_port: "out".into(),
                to_port: "in".into(),
            }],
        );
        let report = validate(&bp);
        assert!(report.is_valid(), "unexpected: {:?}", report.errors());
    }

    #[test]
    fn unknown_node_type_is_an_error() {
        let bp = base(
            vec![Node {
                id: "x".into(),
                r#type: "frobnicate".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            vec![],
        );
        let report = validate(&bp);
        assert!(report
            .errors()
            .iter()
            .any(|d| d.code == "unknown_node_type"));
    }

    #[test]
    fn missing_required_field_is_an_error() {
        let mut plan = task("plan");
        plan.fields.remove("prompt");
        let bp = base(vec![plan], vec![]);
        let report = validate(&bp);
        assert!(report
            .errors()
            .iter()
            .any(|d| d.code == "missing_required_field" && d.node.as_deref() == Some("plan")));
    }

    #[test]
    fn edge_to_unknown_node_is_an_error() {
        let bp = base(
            vec![task("plan")],
            vec![Edge {
                from: "plan".into(),
                to: "ghost".into(),
                from_port: "out".into(),
                to_port: "in".into(),
            }],
        );
        let report = validate(&bp);
        assert!(report.errors().iter().any(|d| d.code == "dangling_edge"));
    }

    #[test]
    fn cycle_is_an_error_because_graph_must_be_a_dag() {
        let bp = base(
            vec![task("a"), task("b")],
            vec![
                Edge {
                    from: "a".into(),
                    to: "b".into(),
                    from_port: "out".into(),
                    to_port: "in".into(),
                },
                Edge {
                    from: "b".into(),
                    to: "a".into(),
                    from_port: "out".into(),
                    to_port: "in".into(),
                },
            ],
        );
        let report = validate(&bp);
        assert!(report.errors().iter().any(|d| d.code == "cycle_detected"));
    }

    #[test]
    fn parent_must_reference_a_loop_node() {
        let mut child = task("child");
        child.parent = Some("plan".into());
        let bp = base(vec![task("plan"), child], vec![]);
        let report = validate(&bp);
        assert!(report.errors().iter().any(|d| d.code == "invalid_parent"));
    }
}
