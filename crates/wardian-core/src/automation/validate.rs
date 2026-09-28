use crate::automation::blueprint::Blueprint;
use crate::automation::condition::validate_path;
use crate::automation::field_type::FieldType;
use crate::automation::registry::find_node_type;
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

    fn warning(code: &'static str, message: impl Into<String>, node: Option<&str>) -> Self {
        Self {
            severity: Severity::Warning,
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

// Delivery preflight happens before tasks. Its inputs can only come from the
// invocation or the persisted run identity, never a reviewer-selected target.
fn validate_delivery_template(value: &str) -> Result<(), &'static str> {
    if value.trim().is_empty() {
        return Err("expected a nonempty delivery field");
    }
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or("unterminated interpolation")?;
        let path = after[..end].trim();
        validate_path(path)?;
        if path != "run.id" && !path.starts_with("trigger.output.") {
            return Err("delivery fields only support trigger.output values and reserved run.id");
        }
        rest = &after[end + 2..];
    }
    Ok(())
}

/// Validate a blueprint against the registry and the structural rules
/// (DAG-only, declared ports, container parents). Returns every finding so the
/// builder can surface them all; the engine refuses to run when `is_valid()` is
/// false.
pub fn validate(blueprint: &Blueprint) -> ValidationReport {
    let mut report = ValidationReport::default();
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
                    "unknown node type `{}` (see `wardian automation node-types`)",
                    node.r#type
                ),
                Some(&node.id),
            ));
            continue;
        };

        if !def.supported {
            report.diagnostics.push(Diagnostic::error(
                "unsupported_node_type",
                format!(
                    "node type `{}` is registered but not supported by the automation runtime",
                    node.r#type
                ),
                Some(&node.id),
            ));
            continue;
        }

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
                if node.r#type == "loop" && field.id == "max_iterations" {
                    if let Some(msg) = check_loop_max_iterations(value) {
                        report.diagnostics.push(Diagnostic::warning(
                            "invalid_loop_max_iterations",
                            format!("node `{}` field `max_iterations`: {}", node.id, msg),
                            Some(&node.id),
                        ));
                    }
                    continue;
                }
                if (node.r#type == "branch" && field.id == "condition")
                    || (node.r#type == "loop" && field.id == "until")
                {
                    if let Some(condition) = value.as_str() {
                        if let Err(message) = validate_path(condition) {
                            report.diagnostics.push(Diagnostic::error(
                                "invalid_condition",
                                format!("node `{}` field `{}`: {}", node.id, field.id, message),
                                Some(&node.id),
                            ));
                        }
                    }
                }
                if let Some(msg) = check_value_kind(&field.field_type, value) {
                    report.diagnostics.push(Diagnostic::error(
                        "invalid_field_value",
                        format!("node `{}` field `{}`: {}", node.id, field.id, msg),
                        Some(&node.id),
                    ));
                }
            }
        }

        if node.r#type == "decision" {
            validate_decision_choices(&mut report, blueprint, node);
        }

        if node.r#type == "message_send" {
            for key in ["recipient", "artifact_path"] {
                if let Some(value) = node.fields.get(key).and_then(|v| v.as_str()) {
                    if let Err(message) = validate_delivery_template(value) {
                        report.diagnostics.push(Diagnostic::error(
                            "invalid_delivery_template",
                            format!("node `{}` field `{key}`: {message}", node.id),
                            Some(&node.id),
                        ));
                    }
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
            if node.r#type == "loop" && parent_is_loop {
                report.diagnostics.push(Diagnostic::error(
                    "nested_loop_unsupported",
                    format!(
                        "loop node `{}` cannot be nested inside loop `{}` until nested-loop replay is supported",
                        node.id, parent_id
                    ),
                    Some(&node.id),
                ));
            }
        }
    }

    // A loop without a body has no executable transition and would otherwise
    // leave the run active forever after its entry event.
    for node in blueprint.nodes.iter().filter(|node| node.r#type == "loop") {
        if !blueprint
            .nodes
            .iter()
            .any(|child| child.parent.as_deref() == Some(node.id.as_str()))
        {
            report.diagnostics.push(Diagnostic::error(
                "empty_loop_body",
                format!(
                    "loop node `{}` must contain at least one body node",
                    node.id
                ),
                Some(&node.id),
            ));
        }
    }

    // Edges reference existing nodes.
    for edge in &blueprint.edges {
        let Some(from_node) = blueprint.find_node(&edge.from) else {
            report.diagnostics.push(Diagnostic::error(
                "dangling_edge",
                format!(
                    "edge `{}` -> `{}` references a missing node",
                    edge.from, edge.to
                ),
                None,
            ));
            continue;
        };
        let Some(to_node) = blueprint.find_node(&edge.to) else {
            report.diagnostics.push(Diagnostic::error(
                "dangling_edge",
                format!(
                    "edge `{}` -> `{}` references a missing node",
                    edge.from, edge.to
                ),
                None,
            ));
            continue;
        };

        if let Some(def) = find_node_type(&from_node.r#type) {
            if def.supported && !declares_output_port(def, from_node, &edge.from_port) {
                report.diagnostics.push(Diagnostic::error(
                    "unknown_output_port",
                    format!(
                        "edge `{}` -> `{}` uses unknown output port `{}` on node `{}`",
                        edge.from, edge.to, edge.from_port, edge.from
                    ),
                    Some(&edge.from),
                ));
            }
        }
        if let Some(def) = find_node_type(&to_node.r#type) {
            if def.supported && !def.inputs.iter().any(|port| port.id == edge.to_port) {
                report.diagnostics.push(Diagnostic::error(
                    "unknown_input_port",
                    format!(
                        "edge `{}` -> `{}` uses unknown input port `{}` on node `{}`",
                        edge.from, edge.to, edge.to_port, edge.to
                    ),
                    Some(&edge.to),
                ));
            }
        }
    }

    // Every loop body must be reachable from the container's body port. A
    // parent annotation alone does not provide an execution transition.
    for loop_node in blueprint.nodes.iter().filter(|node| node.r#type == "loop") {
        let body: HashSet<&str> = blueprint
            .nodes
            .iter()
            .filter(|node| node.parent.as_deref() == Some(loop_node.id.as_str()))
            .map(|node| node.id.as_str())
            .collect();
        if body.is_empty() {
            continue;
        }
        let mut reachable = HashSet::new();
        let mut frontier: Vec<&str> = blueprint
            .edges
            .iter()
            .filter(|edge| {
                edge.from == loop_node.id
                    && edge.from_port == "body"
                    && body.contains(edge.to.as_str())
            })
            .map(|edge| edge.to.as_str())
            .collect();
        while let Some(node_id) = frontier.pop() {
            if !reachable.insert(node_id) {
                continue;
            }
            frontier.extend(
                blueprint
                    .edges
                    .iter()
                    .filter(|edge| edge.from == node_id && body.contains(edge.to.as_str()))
                    .map(|edge| edge.to.as_str()),
            );
        }
        if reachable.is_empty() {
            report.diagnostics.push(Diagnostic::error(
                "missing_loop_body_entry",
                format!(
                    "loop node `{}` must connect its `body` port to a body node",
                    loop_node.id
                ),
                Some(&loop_node.id),
            ));
        }
        for node_id in body.difference(&reachable) {
            report.diagnostics.push(Diagnostic::error(
                "unreachable_loop_body",
                format!(
                    "loop body node `{node_id}` is not reachable from loop `{}` body port",
                    loop_node.id
                ),
                Some(node_id),
            ));
        }
        for edge in blueprint
            .edges
            .iter()
            .filter(|edge| body.contains(edge.to.as_str()))
        {
            if edge.from != loop_node.id && !body.contains(edge.from.as_str()) {
                report.diagnostics.push(Diagnostic::error(
                    "loop_body_external_input",
                    format!(
                        "loop body node `{}` cannot receive an inbound edge from outside loop `{}`",
                        edge.to, loop_node.id
                    ),
                    Some(&edge.to),
                ));
            }
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

fn declares_output_port(
    def: &crate::automation::registry::NodeTypeDef,
    node: &crate::automation::blueprint::Node,
    port: &str,
) -> bool {
    if let Some(field) = &def.outputs_from_field {
        return node
            .fields
            .get(field)
            .and_then(serde_json::Value::as_array)
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(port)));
    }
    def.outputs.iter().any(|output| output.id == port)
}

fn is_valid_port_id(port: &str) -> bool {
    let mut chars = port.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn validate_decision_choices(
    report: &mut ValidationReport,
    blueprint: &Blueprint,
    node: &crate::automation::blueprint::Node,
) {
    let Some(choices) = node
        .fields
        .get("choices")
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    if choices.is_empty() {
        report.diagnostics.push(Diagnostic::error(
            "empty_decision_choices",
            format!(
                "decision node `{}` must declare at least one choice",
                node.id
            ),
            Some(&node.id),
        ));
        return;
    }

    let mut seen = HashSet::new();
    for choice in choices {
        let Some(choice) = choice.as_str() else {
            continue;
        };
        if !is_valid_port_id(choice) {
            report.diagnostics.push(Diagnostic::error(
                "invalid_decision_choice",
                format!(
                    "decision node `{}` choice `{choice}` is not a valid output port id",
                    node.id
                ),
                Some(&node.id),
            ));
        }
        if !seen.insert(choice) {
            report.diagnostics.push(Diagnostic::error(
                "duplicate_decision_choice",
                format!(
                    "decision node `{}` declares choice `{choice}` more than once",
                    node.id
                ),
                Some(&node.id),
            ));
        }
        if is_valid_port_id(choice)
            && !blueprint
                .edges
                .iter()
                .any(|edge| edge.from == node.id && edge.from_port == choice)
        {
            report.diagnostics.push(Diagnostic::error(
                "unconnected_decision_choice",
                format!(
                    "decision node `{}` choice `{choice}` has no outgoing edge",
                    node.id
                ),
                Some(&node.id),
            ));
        }
    }
}

fn check_loop_max_iterations(value: &serde_json::Value) -> Option<String> {
    if let Some(n) = value.as_u64() {
        return (n == 0).then_some("expected a positive integer or a {{...}} template".into());
    }

    if let Some(template) = value.as_str() {
        return (!is_single_template(template))
            .then_some("expected a positive integer or a single {{...}} template".into());
    }

    Some("expected a positive integer or a {{...}} template".into())
}

fn is_single_template(value: &str) -> bool {
    let trimmed = value.trim();
    if !trimmed.starts_with("{{") || !trimmed.ends_with("}}") {
        return false;
    }
    let inner = trimmed[2..trimmed.len() - 2].trim();
    !inner.is_empty() && !inner.contains("{{") && !inner.contains("}}")
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
        FieldType::BranchPort => value
            .as_array()
            .is_some_and(|values| values.iter().all(serde_json::Value::is_string))
            .then_some(())
            .map_or(Some("expected an array of strings".into()), |_| None),
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
    use crate::automation::blueprint::{Blueprint, Edge, Node};

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

    fn loop_node(max_iterations: serde_json::Value) -> Node {
        let mut fields = serde_json::Map::new();
        fields.insert("max_iterations".into(), max_iterations);
        Node {
            id: "lp".into(),
            r#type: "loop".into(),
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
    fn registered_but_unsupported_node_type_is_an_error() {
        let bp = base(
            vec![Node {
                id: "child".into(),
                r#type: "sub_automation".into(),
                name: None,
                parent: None,
                fields: serde_json::json!({"automation": "nested"})
                    .as_object()
                    .unwrap()
                    .clone(),
                position: None,
            }],
            vec![],
        );
        let report = validate(&bp);
        assert!(report.errors().iter().any(|diagnostic| {
            diagnostic.code == "unsupported_node_type"
                && diagnostic.node.as_deref() == Some("child")
        }));
    }

    #[test]
    fn branch_rejects_expression_conditions() {
        let bp = base(
            vec![Node {
                id: "route".into(),
                r#type: "branch".into(),
                name: None,
                parent: None,
                fields: serde_json::json!({
                    "condition": "nodes.agent-1.output.decision === 'HEARTBEAT_ACTION'"
                })
                .as_object()
                .unwrap()
                .clone(),
                position: None,
            }],
            vec![],
        );
        let report = validate(&bp);
        let diagnostic = report
            .errors()
            .into_iter()
            .find(|diagnostic| diagnostic.code == "invalid_condition")
            .expect("expression should have a stable condition diagnostic");
        assert_eq!(diagnostic.node.as_deref(), Some("route"));
        assert!(diagnostic
            .message
            .contains("operators and comparisons are not supported"));
    }

    #[test]
    fn branch_accepts_a_registry_path_condition() {
        let bp = base(
            vec![Node {
                id: "route".into(),
                r#type: "branch".into(),
                name: None,
                parent: None,
                fields: serde_json::json!({"condition": "nodes.agent-1.output.ready"})
                    .as_object()
                    .unwrap()
                    .clone(),
                position: None,
            }],
            vec![],
        );
        assert!(validate(&bp).is_valid());
    }

    #[test]
    fn loop_rejects_expression_until_conditions() {
        let mut loop_node = loop_node(serde_json::json!(2));
        loop_node.fields.insert(
            "until".into(),
            serde_json::json!("nodes.worker.output.count > 2"),
        );
        let mut body = task("body");
        body.parent = Some("lp".into());
        let report = validate(&base(
            vec![loop_node, body],
            vec![Edge {
                from: "lp".into(),
                to: "body".into(),
                from_port: "body".into(),
                to_port: "in".into(),
            }],
        ));
        assert!(report.errors().iter().any(|diagnostic| {
            diagnostic.code == "invalid_condition" && diagnostic.node.as_deref() == Some("lp")
        }));
    }

    #[test]
    fn empty_loop_body_is_an_error() {
        let report = validate(&base(vec![loop_node(serde_json::json!(2))], vec![]));
        assert!(report.errors().iter().any(|diagnostic| {
            diagnostic.code == "empty_loop_body" && diagnostic.node.as_deref() == Some("lp")
        }));
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
    fn edge_ports_must_be_declared_by_both_nodes() {
        let bp = base(
            vec![task("plan"), task("next")],
            vec![Edge {
                from: "plan".into(),
                to: "next".into(),
                from_port: "typo".into(),
                to_port: "also-typo".into(),
            }],
        );
        let report = validate(&bp);
        assert!(report
            .errors()
            .iter()
            .any(|diagnostic| diagnostic.code == "unknown_output_port"));
        assert!(report
            .errors()
            .iter()
            .any(|diagnostic| diagnostic.code == "unknown_input_port"));
    }

    #[test]
    fn decision_choices_declare_dynamic_output_ports() {
        let decision = Node {
            id: "choose".into(),
            r#type: "decision".into(),
            name: None,
            parent: None,
            fields: serde_json::json!({
                "agent": "role:coder",
                "prompt": "choose",
                "choices": ["yes"]
            })
            .as_object()
            .unwrap()
            .clone(),
            position: None,
        };
        let bp = base(
            vec![decision, task("next")],
            vec![Edge {
                from: "choose".into(),
                to: "next".into(),
                from_port: "yes".into(),
                to_port: "in".into(),
            }],
        );
        assert!(validate(&bp).is_valid());
    }

    #[test]
    fn decision_choices_reject_empty_duplicate_malformed_and_unconnected_ports() {
        let decision = Node {
            id: "choose".into(),
            r#type: "decision".into(),
            name: None,
            parent: None,
            fields: serde_json::json!({
                "agent": "role:coder",
                "prompt": "choose",
                "choices": ["yes", "yes", "bad choice"]
            })
            .as_object()
            .unwrap()
            .clone(),
            position: None,
        };
        let report = validate(&base(vec![decision], vec![]));
        assert!(report
            .errors()
            .iter()
            .any(|diagnostic| diagnostic.code == "duplicate_decision_choice"));
        assert!(report
            .errors()
            .iter()
            .any(|diagnostic| diagnostic.code == "invalid_decision_choice"));
        assert!(report
            .errors()
            .iter()
            .any(|diagnostic| diagnostic.code == "unconnected_decision_choice"));

        let mut empty = base(
            vec![Node {
                id: "choose".into(),
                r#type: "decision".into(),
                name: None,
                parent: None,
                fields: serde_json::json!({
                    "agent": "role:coder",
                    "prompt": "choose",
                    "choices": []
                })
                .as_object()
                .unwrap()
                .clone(),
                position: None,
            }],
            vec![],
        );
        let report = validate(&empty);
        assert!(report
            .errors()
            .iter()
            .any(|diagnostic| diagnostic.code == "empty_decision_choices"));
        empty.nodes[0]
            .fields
            .insert("choices".into(), serde_json::json!("yes"));
        let report = validate(&empty);
        assert!(report
            .errors()
            .iter()
            .any(|diagnostic| diagnostic.code == "invalid_field_value"));
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

    #[test]
    fn nested_loops_are_rejected_until_their_replay_semantics_exist() {
        let mut outer = loop_node(serde_json::json!(2));
        outer.id = "outer".into();
        let mut inner = loop_node(serde_json::json!(2));
        inner.id = "inner".into();
        inner.parent = Some("outer".into());
        let mut body = task("body");
        body.parent = Some("inner".into());
        let report = validate(&base(
            vec![outer, inner, body],
            vec![
                Edge {
                    from: "outer".into(),
                    to: "inner".into(),
                    from_port: "body".into(),
                    to_port: "in".into(),
                },
                Edge {
                    from: "inner".into(),
                    to: "body".into(),
                    from_port: "body".into(),
                    to_port: "in".into(),
                },
            ],
        ));

        assert!(report.errors().iter().any(|diagnostic| {
            diagnostic.code == "nested_loop_unsupported"
                && diagnostic.node.as_deref() == Some("inner")
        }));
    }

    #[test]
    fn loop_body_rejects_external_inbound_edges() {
        let mut body = task("body");
        body.parent = Some("lp".into());
        let report = validate(&base(
            vec![task("outside"), loop_node(serde_json::json!(2)), body],
            vec![
                Edge {
                    from: "lp".into(),
                    to: "body".into(),
                    from_port: "body".into(),
                    to_port: "in".into(),
                },
                Edge {
                    from: "outside".into(),
                    to: "body".into(),
                    from_port: "out".into(),
                    to_port: "in".into(),
                },
            ],
        ));

        assert!(report.errors().iter().any(|diagnostic| {
            diagnostic.code == "loop_body_external_input"
                && diagnostic.node.as_deref() == Some("body")
        }));
    }

    #[test]
    fn loop_max_iterations_accepts_template_string() {
        let mut child = task("body");
        child.parent = Some("lp".into());
        let bp = base(
            vec![
                loop_node(serde_json::json!("{{trigger.output.max_cycles}}")),
                child,
            ],
            vec![Edge {
                from: "lp".into(),
                to: "body".into(),
                from_port: "body".into(),
                to_port: "in".into(),
            }],
        );

        let report = validate(&bp);

        assert!(
            report.is_valid(),
            "unexpected errors: {:?}",
            report.errors()
        );
        assert!(!report
            .diagnostics
            .iter()
            .any(|d| { d.code == "invalid_field_value" && d.node.as_deref() == Some("lp") }));
    }

    #[test]
    fn loop_max_iterations_warns_for_malformed_template_string() {
        let mut child = task("body");
        child.parent = Some("lp".into());
        let bp = base(
            vec![
                loop_node(serde_json::json!("{{trigger.output.max_cycles")),
                child,
            ],
            vec![Edge {
                from: "lp".into(),
                to: "body".into(),
                from_port: "body".into(),
                to_port: "in".into(),
            }],
        );

        let report = validate(&bp);

        assert!(report.diagnostics.iter().any(|d| {
            d.severity == Severity::Warning
                && d.code == "invalid_loop_max_iterations"
                && d.node.as_deref() == Some("lp")
        }));
        assert!(report.is_valid());
    }

    #[test]
    fn loop_max_iterations_warns_for_non_positive_literal() {
        let mut child = task("body");
        child.parent = Some("lp".into());
        let bp = base(
            vec![loop_node(serde_json::json!(0)), child],
            vec![Edge {
                from: "lp".into(),
                to: "body".into(),
                from_port: "body".into(),
                to_port: "in".into(),
            }],
        );

        let report = validate(&bp);

        assert!(report.diagnostics.iter().any(|d| {
            d.severity == Severity::Warning
                && d.code == "invalid_loop_max_iterations"
                && d.node.as_deref() == Some("lp")
        }));
        assert!(report.is_valid());
    }

    #[test]
    fn loop_body_must_be_reachable_from_body_port() {
        let mut body = task("body");
        body.parent = Some("lp".into());
        let report = validate(&base(vec![loop_node(serde_json::json!(2)), body], vec![]));
        assert!(report
            .errors()
            .iter()
            .any(|diagnostic| diagnostic.code == "missing_loop_body_entry"));
        assert!(report.errors().iter().any(|diagnostic| {
            diagnostic.code == "unreachable_loop_body" && diagnostic.node.as_deref() == Some("body")
        }));
    }
}
