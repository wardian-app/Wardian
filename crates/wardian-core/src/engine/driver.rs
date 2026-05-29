use crate::engine::core::{self, finalize_if_done, is_approval, step};
use crate::engine::event::{Event, EventKind};
use crate::engine::executor::*;
use crate::engine::graph::Graph;
use crate::engine::interpolate::resolve;
use crate::engine::state::{NodeStatus, RunState, RunStatus};
use crate::engine::store::{append_event, read_checkpoint, read_events, write_checkpoint};
use crate::engine::{EngineError, StepError};
use crate::workflow::{Blueprint, Node};
use std::path::Path;

/// The async engine: drives a run by repeatedly consulting the pure core,
/// executing side-effecting nodes through a `StepExecutor`, and persisting each
/// event + checkpoint under `run_root`.
pub struct Engine;

impl Engine {
    /// Start a fresh run and drive it until it completes, fails, or parks on an
    /// approval. Returns the resulting `RunState`.
    pub async fn start(
        bp: &Blueprint,
        trigger: serde_json::Value,
        run_root: &Path,
        exec: &dyn StepExecutor,
    ) -> crate::engine::Result<RunState> {
        let g = Graph::new(bp);
        let mut s = RunState::new(new_run_id(), &bp.id);
        emit(
            run_root,
            &g,
            &mut s,
            EventKind::RunStarted {
                blueprint_id: bp.id.clone(),
                schema: bp.schema,
                trigger,
            },
        )?;
        drive(&g, &mut s, run_root, exec).await?;
        Ok(s)
    }

    /// Resume a parked/crashed run from its on-disk state and keep driving.
    pub async fn resume(
        bp: &Blueprint,
        run_root: &Path,
        exec: &dyn StepExecutor,
    ) -> crate::engine::Result<RunState> {
        let g = Graph::new(bp);
        let mut s = load_state(&g, run_root)?;
        if s.status == RunStatus::AwaitingApproval {
            return Ok(s); // still needs a human; grant_approval drives it onward
        }
        // Re-mark any mid-flight Running (non-loop) nodes back to Pending so they re-dispatch.
        let running: Vec<String> = s
            .nodes
            .iter()
            .filter(|(_, st)| **st == NodeStatus::Running)
            .map(|(id, _)| id.clone())
            .collect();
        for id in running {
            if g.blueprint()
                .find_node(&id)
                .map(|n| n.r#type != "loop")
                .unwrap_or(true)
            {
                s.set_node_status(&id, NodeStatus::Pending);
            }
        }
        drive(&g, &mut s, run_root, exec).await?;
        Ok(s)
    }

    /// Reconstruct `RunState` purely by replaying the event log (no execution).
    pub fn replay(bp: &Blueprint, run_root: &Path) -> crate::engine::Result<RunState> {
        let g = Graph::new(bp);
        let mut s = RunState::new("replay", &bp.id);
        for ev in read_events(run_root)? {
            core::apply(&g, &mut s, &ev)?;
        }
        Ok(s)
    }

    /// Grant approval on a parked run, then continue driving.
    pub async fn grant_approval(
        bp: &Blueprint,
        run_root: &Path,
        node: &str,
        actor: &str,
        note: Option<String>,
        exec: &dyn StepExecutor,
    ) -> crate::engine::Result<RunState> {
        let g = Graph::new(bp);
        let mut s = load_state(&g, run_root)?;
        if s.status != RunStatus::AwaitingApproval {
            return Err(EngineError::NotAwaitingApproval(node.into()));
        }
        emit(
            run_root,
            &g,
            &mut s,
            EventKind::ApprovalGranted {
                node: node.into(),
                actor: actor.into(),
                note,
            },
        )?;
        drive(&g, &mut s, run_root, exec).await?;
        Ok(s)
    }

    /// Reject approval on a parked run (fails the run).
    pub async fn reject_approval(
        bp: &Blueprint,
        run_root: &Path,
        node: &str,
        actor: &str,
        note: Option<String>,
    ) -> crate::engine::Result<RunState> {
        let g = Graph::new(bp);
        let mut s = load_state(&g, run_root)?;
        if s.status != RunStatus::AwaitingApproval {
            return Err(EngineError::NotAwaitingApproval(node.into()));
        }
        emit(
            run_root,
            &g,
            &mut s,
            EventKind::ApprovalRejected {
                node: node.into(),
                actor: actor.into(),
                note,
            },
        )?;
        Ok(s)
    }
}

fn new_run_id() -> String {
    format!(
        "{}-{}",
        chrono::Utc::now().timestamp_millis(),
        &uuid::Uuid::new_v4().to_string()[..8]
    )
}

fn load_state(g: &Graph<'_>, run_root: &Path) -> crate::engine::Result<RunState> {
    if let Some(s) = read_checkpoint(run_root)? {
        return Ok(s);
    }
    // No checkpoint: rebuild from the log.
    let mut s = RunState::new("rebuilt", &g.blueprint().id);
    for ev in read_events(run_root)? {
        core::apply(g, &mut s, &ev)?;
    }
    Ok(s)
}

/// Emit: stamp seq, fold via `apply`, append to log, checkpoint.
fn emit(
    run_root: &Path,
    g: &Graph<'_>,
    s: &mut RunState,
    kind: EventKind,
) -> crate::engine::Result<()> {
    let ev = Event::new(s.next_seq, kind);
    core::apply(g, s, &ev)?;
    append_event(run_root, &ev)?;
    write_checkpoint(run_root, s)?;
    Ok(())
}

/// The main loop: advance loops, finalize, then dispatch each runnable node.
async fn drive(
    g: &Graph<'_>,
    s: &mut RunState,
    run_root: &Path,
    exec: &dyn StepExecutor,
) -> crate::engine::Result<()> {
    loop {
        core::advance_loops(g, s);
        finalize_if_done(g, s);
        write_checkpoint(run_root, s)?;
        if s.status != RunStatus::Running {
            if s.status == RunStatus::Completed {
                emit(run_root, g, s, EventKind::RunCompleted)?;
            }
            return Ok(());
        }
        let runnable = step(g, s);
        if runnable.is_empty() {
            // No progress possible but not finalized: guard against a stuck graph.
            return Ok(());
        }
        for node_id in runnable {
            dispatch(g, s, run_root, exec, &node_id).await?;
            if s.status == RunStatus::AwaitingApproval || s.status == RunStatus::Failed {
                return Ok(());
            }
        }
    }
}

/// Execute one runnable node: control nodes in-engine, side-effecting via the executor.
async fn dispatch(
    g: &Graph<'_>,
    s: &mut RunState,
    run_root: &Path,
    exec: &dyn StepExecutor,
    node_id: &str,
) -> crate::engine::Result<()> {
    let node = g
        .blueprint()
        .find_node(node_id)
        .ok_or_else(|| EngineError::InvalidState(format!("missing node {node_id}")))?
        .clone();

    // Triggers + join: pass-through completion.
    if node.r#type.ends_with("_trigger") || node.r#type == "manual_trigger" || node.r#type == "join"
    {
        emit(
            run_root,
            g,
            s,
            EventKind::NodeCompleted {
                node: node.id.clone(),
                output: serde_json::json!({}),
            },
        )?;
        return Ok(());
    }
    if is_approval(g, node_id) {
        emit(
            run_root,
            g,
            s,
            EventKind::AwaitingApproval {
                node: node.id.clone(),
            },
        )?;
        return Ok(());
    }
    if node.r#type == "loop" {
        emit_loop_enter(run_root, g, s, node_id)?;
        return Ok(());
    }
    if node.r#type == "branch" {
        let port = eval_branch(s, &node)?;
        emit(
            run_root,
            g,
            s,
            EventKind::BranchTaken {
                node: node.id.clone(),
                port,
            },
        )?;
        return Ok(());
    }

    emit(
        run_root,
        g,
        s,
        EventKind::NodeStarted {
            node: node.id.clone(),
        },
    )?;
    let result = run_side_effect(g, s, exec, &node).await;
    match result {
        Ok(out) => emit(
            run_root,
            g,
            s,
            EventKind::NodeCompleted {
                node: node.id.clone(),
                output: out,
            },
        )?,
        Err(StepError(e)) => emit(
            run_root,
            g,
            s,
            EventKind::NodeFailed {
                node: node.id.clone(),
                error: e,
            },
        )?,
    }
    Ok(())
}

/// Loop entry must record the iteration event AND run the core `enter_loop` side
/// effects (status/ports). We emit `LoopIteration{0}` (folded by apply) then
/// pulse the body via the core helper, persisting the resulting state.
fn emit_loop_enter(
    run_root: &Path,
    g: &Graph<'_>,
    s: &mut RunState,
    loop_id: &str,
) -> crate::engine::Result<()> {
    let ev = Event::new(
        s.next_seq,
        EventKind::LoopIteration {
            node: loop_id.into(),
            iteration: 0,
        },
    );
    core::apply(g, s, &ev)?;
    append_event(run_root, &ev)?;
    core::enter_loop(g, s, loop_id);
    write_checkpoint(run_root, s)?;
    Ok(())
}

fn eval_branch(s: &RunState, node: &Node) -> crate::engine::Result<String> {
    // Minimal condition: truthiness of a registry path named in `condition`.
    let cond = node
        .fields
        .get("condition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let truthy = lookup_truthy(&s.registry, cond);
    Ok(if truthy {
        "on_true".into()
    } else {
        "on_false".into()
    })
}

fn lookup_truthy(registry: &serde_json::Value, path: &str) -> bool {
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

/// Interpolate the node's string fields and call the matching executor method.
async fn run_side_effect(
    g: &Graph<'_>,
    s: &RunState,
    exec: &dyn StepExecutor,
    node: &Node,
) -> Result<serde_json::Value, StepError> {
    let _ = g;
    let f = |key: &str| -> Result<String, StepError> {
        let raw = node.fields.get(key).and_then(|v| v.as_str()).unwrap_or("");
        resolve(raw, &s.registry).map_err(|p| StepError::new(format!("unresolved {{{{{p}}}}}")))
    };
    match node.r#type.as_str() {
        "task" => Ok(exec
            .run_agent_task(AgentTaskRequest {
                node: node.id.clone(),
                agent: f("agent")?,
                prompt: f("prompt")?,
                output_schema: node
                    .fields
                    .get("output_schema")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
            .await?
            .0),
        "decision" => {
            let choices = node
                .fields
                .get("choices")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let port = exec
                .run_decision(DecisionRequest {
                    node: node.id.clone(),
                    agent: f("agent")?,
                    prompt: f("prompt")?,
                    choices,
                })
                .await?
                .0;
            // Encode the chosen port as output; routing handled by DecisionMade in a follow-up.
            Ok(serde_json::json!({ "chosen": port }))
        }
        "shell" => Ok(exec
            .run_shell(ShellRequest {
                node: node.id.clone(),
                command: f("command")?,
                cwd: node
                    .fields
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
            .await?
            .0),
        "script" => Ok(exec
            .run_script(ScriptRequest {
                node: node.id.clone(),
                runtime: f("runtime")?,
                path: f("path")?,
            })
            .await?
            .0),
        "notify" => {
            exec.notify(NotifyRequest {
                node: node.id.clone(),
                message: f("message")?,
            })
            .await?;
            Ok(serde_json::json!({}))
        }
        "state" => Ok(exec
            .state_op(StateRequest {
                node: node.id.clone(),
                op: f("op")?,
                entries: node
                    .fields
                    .get("entries")
                    .cloned()
                    .unwrap_or(serde_json::json!({})),
            })
            .await?
            .0),
        other => Err(StepError::new(format!(
            "no executor for node type `{other}`"
        ))),
    }
}
