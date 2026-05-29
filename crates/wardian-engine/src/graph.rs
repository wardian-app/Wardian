use wardian_core::workflow::Blueprint;

/// Precomputed adjacency over a blueprint's edges, addressed by edge index so
/// `RunState` can track delivered/skipped edges compactly.
pub struct Graph<'a> {
    bp: &'a Blueprint,
}

impl<'a> Graph<'a> {
    pub fn new(bp: &'a Blueprint) -> Self {
        Self { bp }
    }

    pub fn blueprint(&self) -> &Blueprint {
        self.bp
    }

    /// Indices of edges whose `to` is `node`.
    pub fn inbound(&self, node: &str) -> Vec<usize> {
        self.bp
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.to == node)
            .map(|(i, _)| i)
            .collect()
    }

    /// Indices of edges leaving `node` from `port`.
    pub fn outbound_from_port(&self, node: &str, port: &str) -> Vec<usize> {
        self.bp
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.from == node && e.from_port == port)
            .map(|(i, _)| i)
            .collect()
    }

    /// All edge indices leaving `node` (any port).
    pub fn outbound(&self, node: &str) -> Vec<usize> {
        self.bp
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.from == node)
            .map(|(i, _)| i)
            .collect()
    }

    /// Trigger nodes that have no inbound edges (run entry points).
    pub fn entry_nodes(&self) -> Vec<String> {
        self.bp
            .nodes
            .iter()
            .filter(|nd| self.inbound(&nd.id).is_empty())
            .map(|nd| nd.id.clone())
            .collect()
    }

    /// Body subgraph members of a loop node (nodes whose `parent` is `loop_id`).
    pub fn body_nodes(&self, loop_id: &str) -> Vec<String> {
        self.bp
            .nodes
            .iter()
            .filter(|nd| nd.parent.as_deref() == Some(loop_id))
            .map(|nd| nd.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::workflow::{Blueprint, Edge, Node};

    fn n(id: &str, ty: &str, parent: Option<&str>) -> Node {
        Node {
            id: id.into(),
            r#type: ty.into(),
            name: None,
            parent: parent.map(str::to_string),
            fields: serde_json::Map::new(),
            position: None,
        }
    }

    fn e(from: &str, fp: &str, to: &str) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            from_port: fp.into(),
            to_port: "in".into(),
        }
    }

    fn bp() -> Blueprint {
        Blueprint {
            schema: 2,
            id: "wf".into(),
            name: "wf".into(),
            nodes: vec![
                n("t", "manual_trigger", None),
                n("a", "task", None),
                n("lp", "loop", None),
                n("b", "task", Some("lp")),
            ],
            edges: vec![
                e("t", "out", "a"),
                e("a", "out", "lp"),
                e("lp", "body", "b"),
            ],
            body: String::new(),
        }
    }

    #[test]
    fn inbound_and_outbound_by_index() {
        let blueprint = bp();
        let g = Graph::new(&blueprint);
        assert_eq!(g.inbound("a"), vec![0]); // edge t->a
        assert_eq!(g.outbound_from_port("lp", "body"), vec![2]); // lp--body-->b
        assert!(g.inbound("t").is_empty());
    }

    #[test]
    fn entry_nodes_are_triggers_without_inbound() {
        let blueprint = bp();
        let g = Graph::new(&blueprint);
        assert_eq!(g.entry_nodes(), vec!["t".to_string()]);
    }

    #[test]
    fn body_nodes_of_loop() {
        let blueprint = bp();
        let g = Graph::new(&blueprint);
        assert_eq!(g.body_nodes("lp"), vec!["b".to_string()]);
    }
}
