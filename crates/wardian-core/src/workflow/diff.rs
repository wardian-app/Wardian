use crate::workflow::blueprint::Blueprint;
use serde::Serialize;
use std::collections::BTreeMap;

/// A structural diff between two blueprints, keyed by node id. Node ids are the
/// stable identity; everything else is compared by value.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct BlueprintDiff {
    pub added_nodes: Vec<String>,
    pub removed_nodes: Vec<String>,
    pub changed_nodes: Vec<String>,
}

impl BlueprintDiff {
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.changed_nodes.is_empty()
    }
}

/// Compute the node-level diff from `before` to `after`. Output vectors are
/// sorted for deterministic results.
pub fn diff(before: &Blueprint, after: &Blueprint) -> BlueprintDiff {
    let before_map: BTreeMap<&str, &crate::workflow::blueprint::Node> =
        before.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let after_map: BTreeMap<&str, &crate::workflow::blueprint::Node> =
        after.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut d = BlueprintDiff::default();
    for (id, node) in &after_map {
        match before_map.get(id) {
            None => d.added_nodes.push((*id).to_string()),
            Some(prev) if prev != node => d.changed_nodes.push((*id).to_string()),
            Some(_) => {}
        }
    }
    for id in before_map.keys() {
        if !after_map.contains_key(id) {
            d.removed_nodes.push((*id).to_string());
        }
    }
    d.added_nodes.sort();
    d.removed_nodes.sort();
    d.changed_nodes.sort();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::blueprint::{Blueprint, Node};

    fn node(id: &str) -> Node {
        Node {
            id: id.into(),
            r#type: "task".into(),
            name: None,
            parent: None,
            fields: serde_json::Map::new(),
            position: None,
        }
    }
    fn bp(nodes: Vec<Node>) -> Blueprint {
        Blueprint {
            schema: 2,
            id: "d".into(),
            name: "D".into(),
            nodes,
            edges: vec![],
            body: String::new(),
        }
    }

    #[test]
    fn detects_added_and_removed_nodes() {
        let before = bp(vec![node("a"), node("b")]);
        let after = bp(vec![node("b"), node("c")]);
        let d = diff(&before, &after);
        assert_eq!(d.added_nodes, vec!["c".to_string()]);
        assert_eq!(d.removed_nodes, vec!["a".to_string()]);
    }

    #[test]
    fn detects_changed_node() {
        let before = bp(vec![node("a")]);
        let mut changed = node("a");
        changed.name = Some("renamed".into());
        let after = bp(vec![changed]);
        let d = diff(&before, &after);
        assert_eq!(d.changed_nodes, vec!["a".to_string()]);
    }

    #[test]
    fn identical_blueprints_have_empty_diff() {
        let before = bp(vec![node("a")]);
        let after = bp(vec![node("a")]);
        assert!(diff(&before, &after).is_empty());
    }
}
