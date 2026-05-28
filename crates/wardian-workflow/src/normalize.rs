use crate::blueprint::Blueprint;

/// Put a blueprint into canonical form so equivalent graphs serialize
/// identically: nodes sorted by id, edges sorted by (from, from_port, to,
/// to_port). This makes diffs stable and round-trips deterministic.
pub fn normalize(blueprint: &mut Blueprint) {
    blueprint.nodes.sort_by(|a, b| a.id.cmp(&b.id));
    blueprint.edges.sort_by(|a, b| {
        (&a.from, &a.from_port, &a.to, &a.to_port)
            .cmp(&(&b.from, &b.from_port, &b.to, &b.to_port))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{Blueprint, Edge, Node};

    fn node(id: &str, ty: &str) -> Node {
        Node {
            id: id.into(),
            r#type: ty.into(),
            name: None,
            parent: None,
            fields: serde_json::Map::new(),
            position: None,
        }
    }

    #[test]
    fn sorts_nodes_and_edges_deterministically() {
        let mut bp = Blueprint {
            schema: 2,
            id: "demo".into(),
            name: "Demo".into(),
            nodes: vec![node("b", "task"), node("a", "task")],
            edges: vec![
                Edge {
                    from: "b".into(),
                    to: "a".into(),
                    from_port: "out".into(),
                    to_port: "in".into(),
                },
                Edge {
                    from: "a".into(),
                    to: "b".into(),
                    from_port: "out".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        normalize(&mut bp);
        assert_eq!(bp.nodes[0].id, "a");
        assert_eq!(bp.edges[0].from, "a");
    }

    #[test]
    fn normalize_is_idempotent() {
        let mut bp = Blueprint {
            schema: 2,
            id: "demo".into(),
            name: "Demo".into(),
            nodes: vec![node("a", "task")],
            edges: vec![],
            body: String::new(),
        };
        normalize(&mut bp);
        let once = bp.clone();
        normalize(&mut bp);
        assert_eq!(once, bp);
    }
}
