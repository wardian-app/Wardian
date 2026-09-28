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
            // Loop members enter only through their container's `body` port.
            // Treating a parented node with no inbound edge as a top-level
            // entry would execute it outside the loop before the first
            // iteration.
            if nd.parent.is_some() {
                continue;
            }
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
        EventKind::RunStarted {
            run_id,
            blueprint_hash,
            blueprint_id,
            schema,
            trigger,
            ..
        } => {
            if blueprint_id != &g.blueprint().id {
                return Err(crate::engine::EngineError::InvalidState(format!(
                    "automation event belongs to blueprint `{blueprint_id}`, requested `{}`",
                    g.blueprint().id
                )));
            }
            if *schema != g.blueprint().schema {
                return Err(crate::engine::EngineError::InvalidState(format!(
                    "automation event schema is {schema}, requested blueprint schema is {}",
                    g.blueprint().schema
                )));
            }
            if let Some(run_id) = run_id {
                if s.next_seq == 0
                    && !matches!(s.run_id.as_str(), "replay" | "rebuilt")
                    && s.run_id != *run_id
                {
                    return Err(crate::engine::EngineError::InvalidState(format!(
                        "automation run identity changed from `{}` to `{run_id}`",
                        s.run_id
                    )));
                }
                s.run_id = run_id.clone();
            }
            if let Some(blueprint_hash) = blueprint_hash {
                s.blueprint_hash = Some(blueprint_hash.clone());
            }
            s.normalize_legacy();
            s.set_trigger(runtime_trigger_output(trigger, &ev.ts));
        }
        EventKind::MessageSendPrepared {
            node,
            recipient_id,
            artifact_path,
        } => {
            let binding = crate::engine::state::MessageDeliveryBinding {
                recipient_id: recipient_id.clone(),
                artifact_path: artifact_path.clone(),
            };
            if s.message_deliveries
                .get(node)
                .is_some_and(|previous| previous != &binding)
            {
                return Err(crate::engine::EngineError::InvalidState(
                    "message_send binding cannot change within a run".into(),
                ));
            }
            s.message_deliveries.insert(node.clone(), binding);
        }
        EventKind::NodeStarted { node } => s.set_node_status(node, NodeStatus::Running),
        EventKind::NodeCompleted { node, output } => {
            s.set_node_output(node, output.clone());
            s.set_node_status(node, NodeStatus::Completed);
            // Decision nodes route through their durable completion event;
            // they do not have a normal `out` port.
            if g.blueprint()
                .find_node(node)
                .map(|definition| definition.r#type != "decision")
                .unwrap_or(true)
            {
                deliver_from_port(g, s, node, "out");
            }
        }
        EventKind::DecisionCompleted { node, output, port } => {
            s.set_node_output(node, output.clone());
            s.set_node_status(node, NodeStatus::Completed);
            deliver_chosen_port(g, s, node, port);
        }
        EventKind::StateUpdated { op, entries, .. } => {
            apply_state_update(s, op, entries)?;
        }
        EventKind::Notification { .. } => {}
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
            apply_loop_iteration(g, s, node, *iteration)?;
        }
        EventKind::LoopCompleted { node } => {
            s.set_node_status(node, NodeStatus::Completed);
            deliver_from_port(g, s, node, "done");
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

fn apply_state_update(
    s: &mut RunState,
    op: &str,
    entries: &serde_json::Value,
) -> crate::engine::Result<()> {
    let Some(entries) = entries.as_object() else {
        return Err(crate::engine::EngineError::InvalidState(
            "state entries must be an object".into(),
        ));
    };
    let Some(storage) = s.registry.get_mut("storage") else {
        return Err(crate::engine::EngineError::InvalidState(
            "run registry is missing storage".into(),
        ));
    };
    let Some(storage) = storage.as_object_mut() else {
        return Err(crate::engine::EngineError::InvalidState(
            "run registry storage must be an object".into(),
        ));
    };
    match op {
        "set" | "merge" => {
            for (key, value) in entries {
                storage.insert(key.clone(), value.clone());
            }
        }
        "delete" => {
            for key in entries.keys() {
                storage.remove(key);
            }
        }
        other => {
            return Err(crate::engine::EngineError::InvalidState(format!(
                "unknown state op: {other}"
            )))
        }
    }
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
            s.skipped_edges.remove(&i);
            s.delivered.entry(e.to.clone()).or_default().insert(i);
            // A loop may have pulsed another port earlier, causing a
            // successor to be provisionally skipped. A later selected port
            // makes that successor runnable again.
            if s.status_or_pending(&e.to) == NodeStatus::Skipped {
                s.set_node_status(&e.to, NodeStatus::Pending);
            }
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

fn resolve_u32_field(
    fields: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    registry: &serde_json::Value,
    default: u32,
) -> u32 {
    let Some(value) = fields.get(key) else {
        return default;
    };

    if let Some(n) = value.as_u64() {
        return u32::try_from(n).unwrap_or(u32::MAX);
    }

    value
        .as_str()
        .and_then(|template| crate::engine::interpolate::resolve(template, registry).ok())
        .and_then(|resolved| resolved.trim().parse::<u64>().ok())
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        .unwrap_or(default)
}

/// For each Running loop whose body is fully terminal, evaluate its bound and
/// either start the next iteration or finish (pulse `done`).
///
/// The condition is validated here as well as during blueprint validation so
/// direct engine callers cannot turn an unsupported expression into a false
/// condition.
pub fn advance_loops(g: &Graph, s: &RunState) -> crate::engine::Result<Vec<EventKind>> {
    let mut events = Vec::new();
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
            events.push(EventKind::LoopCompleted { node: lp });
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
            .map(|nd| resolve_u32_field(&nd.fields, "max_iterations", &s.registry, 1))
            .unwrap_or(1)
            .max(1);
        let until_met = match loop_node.and_then(|nd| nd.fields.get("until")) {
            None => false,
            Some(value) => {
                let condition = value.as_str().ok_or_else(|| {
                    crate::engine::EngineError::InvalidState(format!(
                        "loop `{lp}` until condition must be a registry path"
                    ))
                })?;
                let condition =
                    crate::automation::condition::validate_path(condition).map_err(|message| {
                        crate::engine::EngineError::InvalidState(format!(
                            "loop `{lp}` until condition is invalid: {message}"
                        ))
                    })?;
                crate::automation::condition::lookup_truthy(&s.registry, &condition)
            }
        };

        if !until_met && iter + 1 < max {
            events.push(EventKind::LoopIteration {
                node: lp,
                iteration: iter + 1,
            });
        } else {
            events.push(EventKind::LoopCompleted { node: lp });
        }
    }
    Ok(events)
}

fn apply_loop_iteration(
    g: &Graph,
    s: &mut RunState,
    loop_id: &str,
    iteration: u32,
) -> crate::engine::Result<()> {
    let body = g.body_nodes(loop_id);
    if iteration == 0 {
        s.set_node_status(loop_id, NodeStatus::Running);
        s.loop_iter.insert(loop_id.to_string(), 0);
        deliver_from_port(g, s, loop_id, "body");
        return Ok(());
    }

    let previous = s.loop_iter.get(loop_id).copied().ok_or_else(|| {
        crate::engine::EngineError::InvalidState(format!(
            "loop `{loop_id}` advanced before its initial iteration"
        ))
    })?;
    if previous + 1 != iteration {
        return Err(crate::engine::EngineError::InvalidState(format!(
            "loop `{loop_id}` expected iteration {}, got {iteration}",
            previous + 1
        )));
    }

    // Snapshot this iteration's outputs as `prev`, then reset the body.
    for node in &body {
        if let Some(output) = s.node_output(node).cloned() {
            s.registry["nodes"][node]["prev"] = output;
        }
        s.set_node_status(node, NodeStatus::Pending);
        s.delivered.remove(node);
    }
    // Clear skipped flags on edges internal to / entering the body.
    let body_set: std::collections::BTreeSet<&str> =
        body.iter().map(|node| node.as_str()).collect();
    for (index, edge) in g.blueprint().edges.iter().enumerate() {
        if body_set.contains(edge.to.as_str())
            && (body_set.contains(edge.from.as_str()) || edge.from == loop_id)
        {
            s.skipped_edges.remove(&index);
        }
    }
    s.loop_iter.insert(loop_id.to_string(), iteration);
    deliver_from_port(g, s, loop_id, "body");
    Ok(())
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
    use crate::automation::{Blueprint, Edge, Node};
    use crate::engine::event::EventKind;
    use crate::engine::graph::Graph;
    use crate::engine::state::{NodeStatus, RunState, RunStatus};

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

    fn enter_loop(g: &Graph, s: &mut RunState, node: &str) {
        let seq = s.next_seq;
        apply(
            g,
            s,
            &crate::engine::event::Event::new(
                seq,
                EventKind::LoopIteration {
                    node: node.into(),
                    iteration: 0,
                },
            ),
        )
        .unwrap();
    }

    fn advance(g: &Graph, s: &mut RunState) {
        let events = advance_loops(g, s).unwrap();
        for kind in events {
            let seq = s.next_seq;
            apply(g, s, &crate::engine::event::Event::new(seq, kind)).unwrap();
        }
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
        advance(&g, &mut s);
        assert_eq!(s.loop_iter["lp"], 1); // continued to iteration 1
        assert_eq!(s.status_or_pending("b"), NodeStatus::Pending); // body reset
                                                                   // iteration 1 body completes -> reaches max (2), so done
        complete(&g, &mut s, "b", serde_json::json!({}));
        advance(&g, &mut s);
        assert_eq!(s.status_or_pending("lp"), NodeStatus::Completed);
        assert!(step(&g, &s).contains(&"ship".to_string())); // done port delivered
    }

    #[test]
    fn loop_uses_interpolated_max_iterations_from_trigger_output() {
        let mut lp = loop_node("lp", 1);
        lp.fields.insert(
            "max_iterations".into(),
            serde_json::json!("{{trigger.output.n}}"),
        );
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
        s.set_trigger(serde_json::json!({ "n": 3 }));
        complete(&g, &mut s, "t", serde_json::json!({}));
        enter_loop(&g, &mut s, "lp");

        complete(&g, &mut s, "b", serde_json::json!({}));
        advance(&g, &mut s);
        assert_eq!(s.loop_iter["lp"], 1);
        assert_eq!(s.status_or_pending("b"), NodeStatus::Pending);

        complete(&g, &mut s, "b", serde_json::json!({}));
        advance(&g, &mut s);
        assert_eq!(s.loop_iter["lp"], 2);
        assert_eq!(s.status_or_pending("b"), NodeStatus::Pending);

        complete(&g, &mut s, "b", serde_json::json!({}));
        advance(&g, &mut s);
        assert_eq!(s.status_or_pending("lp"), NodeStatus::Completed);
        assert!(step(&g, &s).contains(&"ship".to_string()));
    }

    #[test]
    fn loop_keeps_integer_literal_max_iterations_behavior() {
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
        enter_loop(&g, &mut s, "lp");

        complete(&g, &mut s, "b", serde_json::json!({}));
        advance(&g, &mut s);
        assert_eq!(s.loop_iter["lp"], 1);

        complete(&g, &mut s, "b", serde_json::json!({}));
        advance(&g, &mut s);
        assert_eq!(s.status_or_pending("lp"), NodeStatus::Completed);
    }

    #[test]
    fn unresolved_loop_max_iterations_template_falls_back_to_default() {
        let mut lp = loop_node("lp", 2);
        lp.fields.insert(
            "max_iterations".into(),
            serde_json::json!("{{trigger.output.missing}}"),
        );
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

        complete(&g, &mut s, "b", serde_json::json!({}));
        advance(&g, &mut s);

        assert_eq!(s.status_or_pending("lp"), NodeStatus::Completed);
        assert!(step(&g, &s).contains(&"ship".to_string()));
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
        advance(&g, &mut s);

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
        advance(&g, &mut s);

        assert_eq!(s.loop_iter["lp"], 1);
        assert_eq!(s.status_or_pending("b"), NodeStatus::Pending);

        complete(&g, &mut s, "b", serde_json::json!({ "done": false }));
        advance(&g, &mut s);

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

    #[test]
    fn state_updates_are_folded_into_replayable_storage() {
        let blueprint = bp(vec![node("state", "state")], vec![]);
        let g = Graph::new(&blueprint);
        let mut s = RunState::new("r", "wf");

        apply(
            &g,
            &mut s,
            &crate::engine::event::Event::new(
                0,
                EventKind::StateUpdated {
                    node: "state".into(),
                    op: "set".into(),
                    entries: serde_json::json!({"branch": "main", "ready": true}),
                },
            ),
        )
        .unwrap();
        assert_eq!(s.registry["storage"]["branch"], "main");
        assert_eq!(s.registry["storage"]["ready"], true);

        let next_seq = s.next_seq;
        apply(
            &g,
            &mut s,
            &crate::engine::event::Event::new(
                next_seq,
                EventKind::StateUpdated {
                    node: "state".into(),
                    op: "delete".into(),
                    entries: serde_json::json!({"ready": null}),
                },
            ),
        )
        .unwrap();
        assert!(s.registry["storage"].get("ready").is_none());
    }
}
