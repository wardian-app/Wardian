use crate::event::{Event, EventKind};
use crate::graph::Graph;
use crate::state::{NodeStatus, RunState, RunStatus};

/// Nodes that are runnable right now: status Pending, and every inbound edge is
/// resolved (delivered or skipped) with at least one delivered. Trigger/entry
/// nodes (no inbound) are runnable while Pending. Loop bodies/approvals are
/// extended in later tasks.
pub fn step(g: &Graph, s: &RunState) -> Vec<String> {
    if s.status != RunStatus::Running {
        return Vec::new();
    }
    let mut out = Vec::new();
    for nd in &g.blueprint().nodes {
        if s.status_or_pending(&nd.id) != NodeStatus::Pending {
            continue;
        }
        let inbound = g.inbound(&nd.id);
        if inbound.is_empty() {
            out.push(nd.id.clone()); // entry node
            continue;
        }
        let delivered = s.delivered.get(&nd.id);
        let all_resolved = inbound.iter().all(|i| {
            delivered.map(|d| d.contains(i)).unwrap_or(false) || s.skipped_edges.contains(i)
        });
        let any_delivered = inbound
            .iter()
            .any(|i| delivered.map(|d| d.contains(i)).unwrap_or(false));
        if all_resolved && any_delivered {
            out.push(nd.id.clone());
        }
    }
    out
}

/// Fold one event into state. Total and deterministic: replaying the log via
/// `apply` reconstructs `RunState` exactly.
pub fn apply(g: &Graph, s: &mut RunState, ev: &Event) -> crate::Result<()> {
    match &ev.kind {
        EventKind::RunStarted { trigger, .. } => {
            s.set_trigger(trigger.clone());
        }
        EventKind::NodeStarted { node } => s.set_node_status(node, NodeStatus::Running),
        EventKind::NodeCompleted { node, output } => {
            s.set_node_output(node, output.clone());
            s.set_node_status(node, NodeStatus::Completed);
            deliver_from_port(g, s, node, "out");
        }
        EventKind::NodeFailed { node, error } => {
            s.set_node_status(node, NodeStatus::Failed);
            s.status = RunStatus::Failed;
            s.failure = Some(format!("{node}: {error}"));
        }
        EventKind::BranchTaken { node, port } | EventKind::DecisionMade { node, port } => {
            s.set_node_status(node, NodeStatus::Completed);
            deliver_chosen_port(g, s, node, port);
        }
        EventKind::NodeSkipped { node } => {
            s.set_node_status(node, NodeStatus::Skipped);
            // skip all outbound edges -> may cascade to downstream skips
            for i in g.outbound(node) {
                s.skipped_edges.insert(i);
            }
        }
        EventKind::RunCompleted => s.status = RunStatus::Completed,
        EventKind::RunFailed { error } => {
            s.status = RunStatus::Failed;
            s.failure = Some(error.clone());
        }
        // Loop + approval kinds handled in Tasks 8/9.
        _ => {}
    }
    s.next_seq = ev.seq + 1;
    Ok(())
}

/// Deliver the node's single named port (used for normal "out" completion).
fn deliver_from_port(g: &Graph, s: &mut RunState, node: &str, port: &str) {
    for i in g.outbound(node) {
        let e = &g.blueprint().edges[i];
        if e.from_port == port {
            s.delivered.entry(e.to.clone()).or_default().insert(i);
        } else {
            s.skipped_edges.insert(i);
        }
    }
}

/// Deliver only `chosen` port; mark the others' edges skipped (branch/decision).
fn deliver_chosen_port(g: &Graph, s: &mut RunState, node: &str, chosen: &str) {
    deliver_from_port(g, s, node, chosen);
}

/// If nothing is runnable and no node is Running, mark the run Completed (unless
/// already terminal). Cascade skips for any unreachable Pending nodes whose every
/// inbound edge is skipped.
pub fn finalize_if_done(g: &Graph, s: &mut RunState) {
    if s.status != RunStatus::Running {
        return;
    }
    // Cascade: any Pending node with all inbound skipped becomes Skipped.
    loop {
        let mut changed = false;
        let to_skip: Vec<String> = g
            .blueprint()
            .nodes
            .iter()
            .filter(|nd| s.status_or_pending(&nd.id) == NodeStatus::Pending)
            .filter(|nd| {
                let inb = g.inbound(&nd.id);
                !inb.is_empty() && inb.iter().all(|i| s.skipped_edges.contains(i))
            })
            .map(|nd| nd.id.clone())
            .collect();
        for id in to_skip {
            s.set_node_status(&id, NodeStatus::Skipped);
            for i in g.outbound(&id) {
                s.skipped_edges.insert(i);
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }
    let any_running = s.nodes.values().any(|st| *st == NodeStatus::Running);
    let any_runnable = !step(g, s).is_empty();
    if !any_running && !any_runnable {
        s.status = RunStatus::Completed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use crate::graph::Graph;
    use crate::state::{NodeStatus, RunState, RunStatus};
    use wardian_workflow::{Blueprint, Edge, Node};

    fn node(id: &str, ty: &str) -> Node {
        let mut fields = serde_json::Map::new();
        if ty == "task" {
            fields.insert("agent".into(), serde_json::json!("role:x"));
            fields.insert("prompt".into(), serde_json::json!("do {{trigger.output.id}}"));
        }
        if ty == "branch" {
            fields.insert("condition".into(), serde_json::json!("nodes.a.output.ok"));
        }
        Node {
            id: id.into(),
            r#type: ty.into(),
            name: None,
            parent: None,
            fields,
            position: None,
        }
    }

    fn edge(from: &str, fp: &str, to: &str) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            from_port: fp.into(),
            to_port: "in".into(),
        }
    }

    fn bp(nodes: Vec<Node>, edges: Vec<Edge>) -> Blueprint {
        Blueprint {
            schema: 2,
            id: "wf".into(),
            name: "wf".into(),
            nodes,
            edges,
            body: String::new(),
        }
    }

    // Helper: apply a node completion (normal "out" routing).
    fn complete(g: &Graph, s: &mut RunState, node: &str, output: serde_json::Value) {
        let seq = s.next_seq;
        apply(
            g,
            s,
            &crate::event::Event::new(
                seq,
                EventKind::NodeCompleted {
                    node: node.into(),
                    output,
                },
            ),
        )
        .unwrap();
    }

    #[test]
    fn trigger_is_the_initial_runnable_node() {
        let blueprint = bp(
            vec![node("t", "manual_trigger"), node("a", "task")],
            vec![edge("t", "out", "a")],
        );
        let g = Graph::new(&blueprint);
        let s = RunState::new("r", "wf");
        let runnable = step(&g, &s);
        assert_eq!(runnable, vec!["t".to_string()]);
    }

    #[test]
    fn downstream_becomes_runnable_after_upstream_completes() {
        let blueprint = bp(
            vec![node("t", "manual_trigger"), node("a", "task")],
            vec![edge("t", "out", "a")],
        );
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");
        complete(&g, &mut s, "t", serde_json::json!({}));
        assert_eq!(step(&g, &s), vec!["a".to_string()]);
    }

    #[test]
    fn join_waits_for_all_inbound() {
        // t -> a, t -> b, a -> j, b -> j (j is a join)
        let blueprint = bp(
            vec![
                node("t", "manual_trigger"),
                node("a", "task"),
                node("b", "task"),
                node("j", "join"),
            ],
            vec![
                edge("t", "out", "a"),
                edge("t", "out", "b"),
                edge("a", "out", "j"),
                edge("b", "out", "j"),
            ],
        );
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");
        complete(&g, &mut s, "t", serde_json::json!({}));
        complete(&g, &mut s, "a", serde_json::json!({}));
        // only b's edge into j is missing
        assert!(!step(&g, &s).contains(&"j".to_string()));
        complete(&g, &mut s, "b", serde_json::json!({}));
        assert!(step(&g, &s).contains(&"j".to_string()));
    }

    #[test]
    fn run_completes_when_all_reachable_nodes_terminal() {
        let blueprint = bp(
            vec![node("t", "manual_trigger"), node("a", "task")],
            vec![edge("t", "out", "a")],
        );
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");
        complete(&g, &mut s, "t", serde_json::json!({}));
        complete(&g, &mut s, "a", serde_json::json!({}));
        // engine marks completion when nothing is runnable and nothing running/pending-reachable
        finalize_if_done(&g, &mut s);
        assert_eq!(s.status, RunStatus::Completed);
    }
}
