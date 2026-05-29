use serde::{Deserialize, Serialize};

/// Optional canvas position (round-tripped but not interpreted by the engine).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// A node in the workflow graph. `fields` holds the authored values, validated
/// against the node type's `FieldDef`s in the registry. `parent` names the
/// containing `loop` node when this node lives inside a loop body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub fields: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
}

fn default_out() -> String {
    "out".to_string()
}
fn default_in() -> String {
    "in".to_string()
}

/// A directed edge between two node ports. Ports default to `out` -> `in`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    #[serde(default = "default_out")]
    pub from_port: String,
    #[serde(default = "default_in")]
    pub to_port: String,
}

/// The full in-memory blueprint: the structured graph plus the human-readable
/// markdown body that follows the front-matter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blueprint {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// Markdown body after the front-matter. Not part of the YAML; populated by
    /// the parser and skipped during YAML (de)serialization.
    #[serde(skip)]
    pub body: String,
}

fn default_schema() -> u32 {
    2
}

impl Blueprint {
    pub fn find_node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_omits_optional_fields_when_empty() {
        let node = Node {
            id: "plan".into(),
            r#type: "task".into(),
            name: None,
            parent: None,
            fields: serde_json::Map::new(),
            position: None,
        };
        let json = serde_json::to_value(&node).unwrap();
        assert_eq!(json["id"], "plan");
        assert_eq!(json["type"], "task");
        assert!(json.get("parent").is_none());
        assert!(json.get("position").is_none());
    }

    #[test]
    fn edge_defaults_ports_when_deserialized_without_them() {
        let edge: Edge = serde_json::from_value(serde_json::json!({
            "from": "plan",
            "to": "implement"
        }))
        .unwrap();
        assert_eq!(edge.from_port, "out");
        assert_eq!(edge.to_port, "in");
    }

    #[test]
    fn blueprint_collects_nodes_and_edges() {
        let bp = Blueprint {
            schema: 2,
            id: "demo".into(),
            name: "Demo".into(),
            nodes: vec![],
            edges: vec![],
            body: String::new(),
        };
        assert_eq!(bp.schema, 2);
        assert!(bp.find_node("missing").is_none());
    }
}
