use crate::engine::event::{Event, EventKind};
use crate::engine::graph::Graph;
use crate::engine::state::{NodeStatus, RunState, RunStatus};

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
pub fn apply(g: &Graph, s: &mut RunState, ev: &Event) -> crate::engine::Result<()> {
    match &ev.kind {
        EventKind::RunStarted { trigger, .. } => {
            s.set_trigger(runtime_trigger_output(trigger, &ev.ts));
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
        EventKind::LoopIteration { node, iteration } => {
            s.loop_iter.insert(node.clone(), *iteration);
        }
        EventKind::AwaitingApproval { node } => {
            s.set_node_status(node, NodeStatus::Running);
            s.status = RunStatus::AwaitingApproval;
        }
        EventKind::ApprovalGranted { node, .. } => {
            s.status = RunStatus::Running;
            s.set_node_status(node, NodeStatus::Completed);
            deliver_from_port(g, s, node, "out");
        }
        EventKind::ApprovalRejected { node, .. } => {
            s.set_node_status(node, NodeStatus::Failed);
            s.status = RunStatus::Failed;
            s.failure = Some(format!("{node}: approval rejected"));
        }
    }
    s.next_seq = ev.seq + 1;
    Ok(())
}

fn runtime_trigger_output(trigger: &serde_json::Value, timestamp: &str) -> serde_json::Value {
    match trigger {
        serde_json::Value::Object(map) => {
            let mut output = map.clone();
            output
                .entry("timestamp".to_string())
                .or_insert_with(|| serde_json::Value::String(timestamp.to_string()));
            serde_json::Value::Object(output)
        }
        other => serde_json::json!({
            "timestamp": timestamp,
            "payload": other,
        }),
    }
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

/// Enter a loop node: mark Running, record iteration 0, pulse its `body` port.
pub fn enter_loop(g: &Graph, s: &mut RunState, loop_id: &str) {
    s.set_node_status(loop_id, NodeStatus::Running);
    s.loop_iter.insert(loop_id.to_string(), 0);
    deliver_from_port(g, s, loop_id, "body");
}

/// For each Running loop whose body is fully terminal, evaluate its bound and
/// either start the next iteration or finish (pulse `done`).
pub fn advance_loops(g: &Graph, s: &mut RunState) {
    let loop_ids: Vec<String> = g
        .blueprint()
        .nodes
        .iter()
        .filter(|nd| nd.r#type == "loop" && s.status_or_pending(&nd.id) == NodeStatus::Running)
        .map(|nd| nd.id.clone())
        .collect();

    for lp in loop_ids {
        let body = g.body_nodes(&lp);
        if body.is_empty() {
            continue;
        }
        let body_terminal = body.iter().all(|b| {
            matches!(
                s.status_or_pending(b),
                NodeStatus::Completed | NodeStatus::Skipped | NodeStatus::Failed
            )
        });
        if !body_terminal {
            continue;
        }

        let iter = *s.loop_iter.get(&lp).unwrap_or(&0);
        let loop_node = g.blueprint().find_node(&lp);
        let max = loop_node
            .and_then(|nd| nd.fields.get("max_iterations"))
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32;
        let until_met = loop_node
            .and_then(|nd| nd.fields.get("until"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|condition| !condition.is_empty())
            .map(|condition| lookup_truthy(&s.registry, condition))
            .unwrap_or(false);

        if !until_met && iter + 1 < max {
            // Snapshot this iteration's outputs as `prev`, then reset the body.
            for b in &body {
                if let Some(out) = s.node_output(b).cloned() {
                    s.registry["nodes"][b]["prev"] = out;
                }
                s.set_node_status(b, NodeStatus::Pending);
                s.delivered.remove(b);
            }
            // Clear skipped flags on edges internal to / entering the body.
            let body_set: std::collections::BTreeSet<&str> =
                body.iter().map(|x| x.as_str()).collect();
            let internal: Vec<usize> = g
                .blueprint()
                .edges
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    body_set.contains(e.to.as_str())
                        && (body_set.contains(e.from.as_str()) || e.from == lp)
                })
                .map(|(i, _)| i)
                .collect();
            for i in internal {
                s.skipped_edges.remove(&i);
            }
            s.loop_iter.insert(lp.clone(), iter + 1);
            deliver_from_port(g, s, &lp, "body");
        } else {
            s.set_node_status(&lp, NodeStatus::Completed);
            deliver_from_port(g, s, &lp, "done");
        }
    }
}

pub(crate) fn lookup_truthy(registry: &serde_json::Value, path: &str) -> bool {
    let mut cur = registry;
    for seg in path.split('.') {
        match cur.get(seg) {
            Some(v) => cur = v,
            None => return false,
        }
    }
    match cur {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Null => false,
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        _ => true,
    }
}

/// True if the node is an approval gate (driver parks instead of executing).
pub fn is_approval(g: &Graph, node: &str) -> bool {
    g.blueprint()
        .find_node(node)
        .map(|n| n.r#type == "approval")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event::EventKind;
    use crate::engine::graph::Graph;
    use crate::engine::state::{NodeStatus, RunState, RunStatus};
    use crate::workflow::{Blueprint, Edge, Node};

    fn node(id: &str, ty: &str) -> Node {
        let mut fields = serde_json::Map::new();
        if ty == "task" {
            fields.insert("agent".into(), serde_json::json!("role:x"));
            fields.insert(
                "prompt".into(),
                serde_json::json!("do {{trigger.output.id}}"),
            );
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
            &crate::engine::event::Event::new(
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

    fn loop_node(id: &str, max: u32) -> Node {
        let mut fields = serde_json::Map::new();
        fields.insert("max_iterations".into(), serde_json::json!(max));
        Node {
            id: id.into(),
            r#type: "loop".into(),
            name: None,
            parent: None,
            fields,
            position: None,
        }
    }

    fn child(id: &str, parent: &str) -> Node {
        let mut fields = serde_json::Map::new();
        fields.insert("agent".into(), serde_json::json!("role:x"));
        fields.insert("prompt".into(), serde_json::json!("work"));
        Node {
            id: id.into(),
            r#type: "task".into(),
            name: None,
            parent: Some(parent.into()),
            fields,
            position: None,
        }
    }

    #[test]
    fn loop_runs_body_then_done_after_max_iterations() {
        // t -> lp ; lp--body-->b ; lp--done-->ship
        let blueprint = bp(
            vec![
                node("t", "manual_trigger"),
                loop_node("lp", 2),
                child("b", "lp"),
                node("ship", "task"),
            ],
            vec![
                edge("t", "out", "lp"),
                edge("lp", "body", "b"),
                edge("lp", "done", "ship"),
            ],
        );
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");
        complete(&g, &mut s, "t", serde_json::json!({}));
        // lp is now runnable; enter it.
        assert!(step(&g, &s).contains(&"lp".to_string()));
        enter_loop(&g, &mut s, "lp");
        assert_eq!(s.loop_iter["lp"], 0);
        assert!(step(&g, &s).contains(&"b".to_string())); // body entry runnable
                                                          // iteration 0 body completes
        complete(&g, &mut s, "b", serde_json::json!({}));
        advance_loops(&g, &mut s);
        assert_eq!(s.loop_iter["lp"], 1); // continued to iteration 1
        assert_eq!(s.status_or_pending("b"), NodeStatus::Pending); // body reset
                                                                   // iteration 1 body completes -> reaches max (2), so done
        complete(&g, &mut s, "b", serde_json::json!({}));
        advance_loops(&g, &mut s);
        assert_eq!(s.status_or_pending("lp"), NodeStatus::Completed);
        assert!(step(&g, &s).contains(&"ship".to_string())); // done port delivered
    }

    #[test]
    fn loop_exits_early_when_until_condition_is_truthy() {
        let mut lp = loop_node("lp", 5);
        lp.fields
            .insert("until".into(), serde_json::json!("nodes.b.output.done"));
        let blueprint = bp(
            vec![
                node("t", "manual_trigger"),
                lp,
                child("b", "lp"),
                node("ship", "task"),
            ],
            vec![
                edge("t", "out", "lp"),
                edge("lp", "body", "b"),
                edge("lp", "done", "ship"),
            ],
        );
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");
        complete(&g, &mut s, "t", serde_json::json!({}));
        enter_loop(&g, &mut s, "lp");

        complete(&g, &mut s, "b", serde_json::json!({ "done": true }));
        advance_loops(&g, &mut s);

        assert_eq!(s.loop_iter["lp"], 0);
        assert_eq!(s.status_or_pending("lp"), NodeStatus::Completed);
        assert!(step(&g, &s).contains(&"ship".to_string()));
    }

    #[test]
    fn loop_continues_until_max_when_until_condition_is_false() {
        let mut lp = loop_node("lp", 2);
        lp.fields
            .insert("until".into(), serde_json::json!("nodes.b.output.done"));
        let blueprint = bp(
            vec![
                node("t", "manual_trigger"),
                lp,
                child("b", "lp"),
                node("ship", "task"),
            ],
            vec![
                edge("t", "out", "lp"),
                edge("lp", "body", "b"),
                edge("lp", "done", "ship"),
            ],
        );
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");
        complete(&g, &mut s, "t", serde_json::json!({}));
        enter_loop(&g, &mut s, "lp");

        complete(&g, &mut s, "b", serde_json::json!({ "done": false }));
        advance_loops(&g, &mut s);

        assert_eq!(s.loop_iter["lp"], 1);
        assert_eq!(s.status_or_pending("b"), NodeStatus::Pending);

        complete(&g, &mut s, "b", serde_json::json!({ "done": false }));
        advance_loops(&g, &mut s);

        assert_eq!(s.status_or_pending("lp"), NodeStatus::Completed);
        assert!(step(&g, &s).contains(&"ship".to_string()));
    }

    #[test]
    fn awaiting_approval_parks_and_grant_routes_out() {
        let blueprint = bp(
            vec![
                node("t", "manual_trigger"),
                node("gate", "approval"),
                node("ship", "task"),
            ],
            vec![edge("t", "out", "gate"), edge("gate", "out", "ship")],
        );
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");
        complete(&g, &mut s, "t", serde_json::json!({}));
        assert!(step(&g, &s).contains(&"gate".to_string()));
        // park
        let seq = s.next_seq;
        apply(
            &g,
            &mut s,
            &crate::engine::event::Event::new(
                seq,
                EventKind::AwaitingApproval {
                    node: "gate".into(),
                },
            ),
        )
        .unwrap();
        assert_eq!(s.status, RunStatus::AwaitingApproval);
        assert!(step(&g, &s).is_empty()); // parked: nothing runnable
                                          // grant -> running again, gate completed, ship runnable
        let seq = s.next_seq;
        apply(
            &g,
            &mut s,
            &crate::engine::event::Event::new(
                seq,
                EventKind::ApprovalGranted {
                    node: "gate".into(),
                    actor: "tan".into(),
                    note: None,
                },
            ),
        )
        .unwrap();
        assert_eq!(s.status, RunStatus::Running);
        assert!(step(&g, &s).contains(&"ship".to_string()));
    }

    #[test]
    fn reject_fails_the_run() {
        let blueprint = bp(
            vec![node("t", "manual_trigger"), node("gate", "approval")],
            vec![edge("t", "out", "gate")],
        );
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");
        complete(&g, &mut s, "t", serde_json::json!({}));
        let seq = s.next_seq;
        apply(
            &g,
            &mut s,
            &crate::engine::event::Event::new(
                seq,
                EventKind::AwaitingApproval {
                    node: "gate".into(),
                },
            ),
        )
        .unwrap();
        let seq = s.next_seq;
        apply(
            &g,
            &mut s,
            &crate::engine::event::Event::new(
                seq,
                EventKind::ApprovalRejected {
                    node: "gate".into(),
                    actor: "tan".into(),
                    note: Some("no".into()),
                },
            ),
        )
        .unwrap();
        assert_eq!(s.status, RunStatus::Failed);
    }
}
