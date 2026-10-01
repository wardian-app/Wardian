use crate::automation::{Blueprint, Node};
use crate::engine::core::{self, finalize_if_done, is_approval, step};
use crate::engine::event::{Event, EventKind};
use crate::engine::executor::*;
use crate::engine::graph::Graph;
use crate::engine::interpolate::resolve;
use crate::engine::state::{NodeStatus, RunState, RunStatus};
use crate::engine::store::{
    append_event, read_checkpoint, read_events, write_blueprint_snapshot, write_checkpoint,
};
use crate::engine::{EngineError, StepError};
use sha2::{Digest, Sha256};
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
        Self::start_with_id(bp, new_run_id(), trigger, run_root, exec).await
    }

    /// Start a fresh run with a caller-supplied run id and drive it until it
    /// completes, fails, or parks on an approval. Returns the resulting
    /// `RunState`.
    pub async fn start_with_id(
        bp: &Blueprint,
        run_id: impl Into<String>,
        trigger: serde_json::Value,
        run_root: &Path,
        exec: &dyn StepExecutor,
    ) -> crate::engine::Result<RunState> {
        let s = Self::initialize_with_id(bp, run_id, trigger, run_root)?;
        Self::drive_from_state(bp, s, run_root, exec).await
    }

    /// Initialize a fresh run by writing the invocation-independent
    /// `RunStarted` event and checkpoint. Callers that detach long-running
    /// execution can use this as the durable startup acknowledgement.
    pub fn initialize_with_id(
        bp: &Blueprint,
        run_id: impl Into<String>,
        trigger: serde_json::Value,
        run_root: &Path,
    ) -> crate::engine::Result<RunState> {
        ensure_valid_blueprint(bp)?;
        let g = Graph::new(bp);
        let mut s = RunState::new(run_id.into(), &bp.id);
        let blueprint_hash = blueprint_hash(bp);
        write_blueprint_snapshot(run_root, bp)?;
        let started_run_id = s.run_id.clone();
        emit(
            run_root,
            &g,
            &mut s,
            EventKind::RunStarted {
                run_id: Some(started_run_id),
                blueprint_hash: Some(blueprint_hash),
                blueprint_id: bp.id.clone(),
                schema: bp.schema,
                trigger,
            },
        )?;
        Ok(s)
    }

    /// Continue driving an already-initialized run state.
    pub async fn drive_from_state(
        bp: &Blueprint,
        mut s: RunState,
        run_root: &Path,
        exec: &dyn StepExecutor,
    ) -> crate::engine::Result<RunState> {
        ensure_valid_blueprint(bp)?;
        ensure_blueprint_matches(bp, &s)?;
        let g = Graph::new(bp);
        drive(&g, &mut s, run_root, exec).await?;
        Ok(s)
    }

    /// Resume a parked/crashed run from its on-disk state and keep driving.
    pub async fn resume(
        bp: &Blueprint,
        run_root: &Path,
        exec: &dyn StepExecutor,
    ) -> crate::engine::Result<RunState> {
        ensure_valid_blueprint(bp)?;
        let g = Graph::new(bp);
        let mut s = load_state(&g, run_root)?;
        ensure_blueprint_matches(bp, &s)?;
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

    /// Cancel a run immediately when it is parked for approval. Running runs
    /// retain the marker for the active driver to consume at its next
    /// cooperative boundary; terminal runs clean up a stale marker.
    pub fn cancel(bp: &Blueprint, run_root: &Path) -> crate::engine::Result<RunState> {
        let _approval_decision = acquire_approval_decision_guard(run_root)?;
        let g = Graph::new(bp);
        let mut s = load_state(&g, run_root)?;
        ensure_blueprint_matches(bp, &s)?;
        match s.status {
            RunStatus::AwaitingApproval => {
                emit(
                    run_root,
                    &g,
                    &mut s,
                    EventKind::RunFailed {
                        error: "automation cancelled by operator".into(),
                    },
                )?;
                clear_cancellation_request(run_root)?;
            }
            RunStatus::Completed | RunStatus::Failed => {
                clear_cancellation_request(run_root)?;
            }
            RunStatus::Running => {}
        }
        Ok(s)
    }

    /// Reconstruct `RunState` purely by replaying the event log (no execution).
    pub fn replay(bp: &Blueprint, run_root: &Path) -> crate::engine::Result<RunState> {
        let g = Graph::new(bp);
        let mut s = RunState::new("replay", &bp.id);
        let events = read_events(run_root)?;
        fold_event_log(&g, &mut s, &events)?;
        ensure_blueprint_matches(bp, &s)?;
        Ok(s)
    }

    /// Replay a scheduler launch failure that was recorded before a blueprint
    /// could be loaded. These artifacts intentionally contain only a terminal
    /// `RunFailed` event, so they do not require a mutable library blueprint.
    pub fn replay_launch_failure(
        blueprint_id: &str,
        run_root: &Path,
    ) -> crate::engine::Result<RunState> {
        let state = read_checkpoint(run_root)?
            .ok_or_else(|| EngineError::InvalidState("automation checkpoint is missing".into()))?;
        if state.blueprint_id != blueprint_id {
            return Err(EngineError::InvalidState(format!(
                "automation checkpoint belongs to blueprint `{}`, requested `{blueprint_id}`",
                state.blueprint_id
            )));
        }
        let events = read_events(run_root)?;
        let Some(Event {
            seq: 0,
            kind: EventKind::RunFailed { error },
            ..
        }) = events.first()
        else {
            return Err(EngineError::InvalidState(
                "automation launch-failure artifact must start with run_failed at sequence 0"
                    .into(),
            ));
        };
        if events.len() != 1 {
            return Err(EngineError::InvalidState(
                "automation launch-failure artifact must contain exactly one event".into(),
            ));
        }
        if state.status != RunStatus::Failed
            || state.next_seq != 1
            || state.failure.as_deref() != Some(error.as_str())
        {
            return Err(EngineError::InvalidState(
                "automation launch-failure artifact does not match its failed checkpoint".into(),
            ));
        }
        Ok(state)
    }

    /// Validate a run event sequence without requiring a blueprint graph.
    pub fn validate_event_sequence(events: &[Event]) -> crate::engine::Result<u64> {
        validate_event_sequence(events)
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
        let s = Self::record_approval_granted(bp, run_root, node, actor, note)?;
        Self::drive_from_state(bp, s, run_root, exec).await
    }

    /// Persist an approval decision without executing the remainder of the
    /// automation. Callers that detach long-running continuation work can use
    /// the returned running state with [`Self::drive_from_state`].
    pub fn record_approval_granted(
        bp: &Blueprint,
        run_root: &Path,
        node: &str,
        actor: &str,
        note: Option<String>,
    ) -> crate::engine::Result<RunState> {
        let _approval_decision = acquire_approval_decision_guard(run_root)?;
        let g = Graph::new(bp);
        let mut s = load_state(&g, run_root)?;
        ensure_valid_blueprint(bp)?;
        ensure_blueprint_matches(bp, &s)?;
        if s.status != RunStatus::AwaitingApproval {
            return Err(EngineError::NotAwaitingApproval(node.into()));
        }
        ensure_approval_node(&g, run_root, node)?;
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
        let _approval_decision = acquire_approval_decision_guard(run_root)?;
        let g = Graph::new(bp);
        let mut s = load_state(&g, run_root)?;
        ensure_valid_blueprint(bp)?;
        ensure_blueprint_matches(bp, &s)?;
        if s.status != RunStatus::AwaitingApproval {
            return Err(EngineError::NotAwaitingApproval(node.into()));
        }
        ensure_approval_node(&g, run_root, node)?;
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

    /// Cancel an approval-parked run using only its durable checkpoint and
    /// event log. This remains available when the mutable library blueprint
    /// has been deleted or moved.
    pub fn cancel_awaiting(run_root: &Path) -> crate::engine::Result<RunState> {
        let _approval_decision = acquire_approval_decision_guard(run_root)?;
        let Some(mut s) = read_checkpoint(run_root)? else {
            return Err(EngineError::InvalidState(
                "automation checkpoint is missing".into(),
            ));
        };
        if s.status != RunStatus::AwaitingApproval {
            return Err(EngineError::NotAwaitingApproval("unknown".into()));
        }
        let events = read_events(run_root)?;
        let event_next_seq = validate_event_sequence(&events)?;
        if s.next_seq > event_next_seq {
            return Err(EngineError::InvalidState(format!(
                "automation checkpoint expects sequence {}, event log ends at {}",
                s.next_seq,
                event_next_seq.saturating_sub(1)
            )));
        }
        if !events.iter().any(|event| {
            matches!(&event.kind, EventKind::RunFailed { error } if error == "automation cancelled by operator")
        }) {
            append_event(
                run_root,
                &Event::new(
                    event_next_seq,
                    EventKind::RunFailed {
                        error: "automation cancelled by operator".into(),
                    },
                ),
            )?;
            s.next_seq = event_next_seq + 1;
        } else {
            s.next_seq = event_next_seq;
        }
        s.status = RunStatus::Failed;
        s.failure = Some("automation cancelled by operator".into());
        write_checkpoint(run_root, &s)?;
        clear_cancellation_request(run_root)?;
        Ok(s)
    }
}

fn acquire_approval_decision_guard(
    run_root: &Path,
) -> crate::engine::Result<crate::automation_approval_lock::ApprovalDecisionGuard> {
    match crate::automation_approval_lock::acquire_approval_decision_guard(run_root) {
        Ok(guard) => Ok(guard),
        Err(crate::automation_approval_lock::ApprovalDecisionLockError::Contended) => {
            Err(EngineError::ApprovalDecisionInProgress)
        }
        Err(crate::automation_approval_lock::ApprovalDecisionLockError::Io(error)) => {
            Err(EngineError::Io(error))
        }
    }
}

pub fn new_run_id() -> String {
    format!(
        "{}-{}",
        chrono::Utc::now().timestamp_millis(),
        &uuid::Uuid::new_v4().to_string()[..8]
    )
}

fn load_state(g: &Graph<'_>, run_root: &Path) -> crate::engine::Result<RunState> {
    let mut s = read_checkpoint(run_root)?.unwrap_or_else(|| {
        // No checkpoint: rebuild from the log.
        RunState::new("rebuilt", &g.blueprint().id)
    });
    let events = read_events(run_root)?;
    fold_event_log(g, &mut s, &events)?;
    Ok(s)
}

/// Stable hash of the parsed automation graph. The markdown body is skipped by
/// `Blueprint` serialization, so documentation-only edits do not invalidate a
/// parked run while graph or field edits do.
pub fn blueprint_hash(bp: &Blueprint) -> String {
    let bytes = serde_json::to_vec(bp).expect("automation blueprints are serializable");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn ensure_valid_blueprint(bp: &Blueprint) -> crate::engine::Result<()> {
    let report = crate::automation::validate(bp);
    if report.is_valid() {
        return Ok(());
    }
    let diagnostics = report
        .errors()
        .into_iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
        .collect::<Vec<_>>()
        .join("; ");
    Err(EngineError::InvalidState(format!(
        "automation blueprint is invalid: {diagnostics}"
    )))
}

fn ensure_blueprint_matches(bp: &Blueprint, state: &RunState) -> crate::engine::Result<()> {
    if state.blueprint_id != bp.id {
        return Err(EngineError::InvalidState(format!(
            "automation blueprint identity mismatch: run belongs to `{}`, requested `{}`",
            state.blueprint_id, bp.id
        )));
    }
    if let Some(expected) = state.blueprint_hash.as_deref() {
        let actual = blueprint_hash(bp);
        if expected != actual {
            return Err(EngineError::InvalidState(format!(
                "automation blueprint changed after run start: expected {expected}, found {actual}"
            )));
        }
    }
    Ok(())
}

fn ensure_approval_node(
    g: &Graph<'_>,
    run_root: &Path,
    requested: &str,
) -> crate::engine::Result<()> {
    if !is_approval(g, requested) {
        return Err(EngineError::InvalidState(format!(
            "approval decision targets non-approval node `{requested}`"
        )));
    }
    let events = read_events(run_root)?;
    let expected = events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            EventKind::AwaitingApproval { node } => Some(node.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            EngineError::InvalidState("awaiting approval node is missing from the event log".into())
        })?;
    if expected != requested {
        return Err(EngineError::InvalidState(format!(
            "approval decision targets `{requested}`, but run is awaiting approval at `{expected}`"
        )));
    }
    Ok(())
}

/// Validate the append-only event sequence once for both replay and resume,
/// then fold only the portion not already represented by the checkpoint.
fn fold_event_log(g: &Graph<'_>, s: &mut RunState, events: &[Event]) -> crate::engine::Result<()> {
    if events.is_empty() {
        return Err(EngineError::InvalidState(
            "automation event log is empty".into(),
        ));
    }
    let checkpoint_next_seq = s.next_seq;
    let event_next_seq = validate_event_sequence(events)?;
    validate_event_identity(g, s, events)?;
    let mut expected = 0u64;
    for ev in events {
        if ev.seq != expected {
            return Err(EngineError::InvalidState(format!(
                "automation event sequence gap: expected {expected}, got {}",
                ev.seq
            )));
        }
        if ev.seq >= checkpoint_next_seq {
            if ev.seq != s.next_seq {
                return Err(EngineError::InvalidState(format!(
                    "automation event sequence gap: expected {}, got {}",
                    s.next_seq, ev.seq
                )));
            }
            core::apply(g, s, ev)?;
        }
        expected += 1;
    }
    if event_next_seq < checkpoint_next_seq {
        return Err(EngineError::InvalidState(format!(
            "automation event log ends at sequence {}, checkpoint expects {}",
            expected.saturating_sub(1),
            checkpoint_next_seq.saturating_sub(1)
        )));
    }
    Ok(())
}

/// Check identity-bearing startup metadata before a checkpoint can hide the
/// beginning of the event log from replay. Checkpoint-less recovery uses the
/// sentinel run id and adopts the durable id from `RunStarted`.
fn validate_event_identity(
    g: &Graph<'_>,
    s: &RunState,
    events: &[Event],
) -> crate::engine::Result<()> {
    let startup = match events.first() {
        Some(Event {
            seq: 0,
            kind:
                EventKind::RunStarted {
                    run_id,
                    blueprint_id,
                    blueprint_hash,
                    schema,
                    ..
                },
            ..
        }) => (run_id, blueprint_id, blueprint_hash, *schema),
        Some(event) => {
            return Err(EngineError::InvalidState(format!(
                "automation event log must start with run_started at sequence 0, found {:?}",
                event.kind
            )))
        }
        None => unreachable!("fold_event_log rejects empty event logs"),
    };

    let (run_id, blueprint_id, event_hash, schema) = startup;
    if blueprint_id != &g.blueprint().id {
        return Err(EngineError::InvalidState(format!(
            "automation event belongs to blueprint `{blueprint_id}`, requested `{}`",
            g.blueprint().id
        )));
    }
    if schema != g.blueprint().schema {
        return Err(EngineError::InvalidState(format!(
            "automation event schema is {schema}, requested blueprint schema is {}",
            g.blueprint().schema
        )));
    }
    if s.blueprint_id != g.blueprint().id {
        return Err(EngineError::InvalidState(format!(
            "automation checkpoint belongs to blueprint `{}`, requested `{}`",
            s.blueprint_id,
            g.blueprint().id
        )));
    }
    if !matches!(s.run_id.as_str(), "replay" | "rebuilt") {
        if let Some(event_run_id) = run_id {
            if event_run_id != &s.run_id {
                return Err(EngineError::InvalidState(format!(
                    "automation checkpoint belongs to run `{}`, event log belongs to `{event_run_id}`",
                    s.run_id
                )));
            }
        }
    }
    if let Some(event_hash) = event_hash {
        if let Some(checkpoint_hash) = s.blueprint_hash.as_deref() {
            if checkpoint_hash != event_hash {
                return Err(EngineError::InvalidState(format!(
                    "automation checkpoint blueprint hash is `{checkpoint_hash}`, event log hash is `{event_hash}`"
                )));
            }
        }
    }
    Ok(())
}

/// Validate that an event log starts at zero and has no gaps or reordering.
pub fn validate_event_sequence(events: &[Event]) -> crate::engine::Result<u64> {
    let mut expected = 0u64;
    for ev in events {
        if ev.seq != expected {
            return Err(EngineError::InvalidState(format!(
                "automation event sequence gap: expected {expected}, got {}",
                ev.seq
            )));
        }
        expected += 1;
    }
    Ok(expected)
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

fn message_send_request(node: &Node, state: &RunState) -> Result<MessageSendRequest, StepError> {
    if let Some(binding) = state.message_deliveries.get(&node.id) {
        return Ok(MessageSendRequest {
            node: node.id.clone(),
            recipient: binding.recipient_id.clone(),
            artifact_path: binding.artifact_path.clone(),
        });
    }
    let field = |key: &str| {
        let raw = node
            .fields
            .get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| StepError::new(format!("missing message_send field `{key}`")))?;
        resolve(raw, &state.registry)
            .map_err(|path| StepError::new(format!("unresolved {{{{{path}}}}}")))
    };
    Ok(MessageSendRequest {
        node: node.id.clone(),
        recipient: field("recipient")?,
        artifact_path: field("artifact_path")?,
    })
}

/// The main loop: advance loops, finalize, then dispatch each runnable node.
async fn drive(
    g: &Graph<'_>,
    s: &mut RunState,
    run_root: &Path,
    exec: &dyn StepExecutor,
) -> crate::engine::Result<()> {
    s.normalize_legacy();
    if s.status == RunStatus::Running {
        let fresh = !g
            .blueprint()
            .nodes
            .iter()
            .any(|node| node.r#type != "manual_trigger" && s.node_status(&node.id).is_some());
        for node in g
            .blueprint()
            .nodes
            .iter()
            .filter(|node| node.r#type == "message_send")
        {
            if matches!(
                s.node_status(&node.id),
                Some(NodeStatus::Completed | NodeStatus::Skipped)
            ) {
                continue;
            }
            let result = match message_send_request(node, s) {
                Ok(req) => match exec.preflight_message_send(req.clone(), fresh).await {
                    Ok(recipient_id) => {
                        if s.message_deliveries
                            .get(&node.id)
                            .is_some_and(|binding| binding.recipient_id != recipient_id)
                        {
                            Err(StepError::new(
                                "bound message_send recipient no longer exists",
                            ))
                        } else {
                            if !s.message_deliveries.contains_key(&node.id) {
                                emit(
                                    run_root,
                                    g,
                                    s,
                                    EventKind::MessageSendPrepared {
                                        node: node.id.clone(),
                                        recipient_id,
                                        artifact_path: req.artifact_path,
                                    },
                                )?;
                            }
                            Ok(())
                        }
                    }
                    Err(err) => Err(err),
                },
                Err(err) => Err(err),
            };
            if let Err(err) = result {
                emit(
                    run_root,
                    g,
                    s,
                    EventKind::RunFailed {
                        error: format!("message_send preflight `{}`: {err}", node.id),
                    },
                )?;
                return Ok(());
            }
        }
    }
    loop {
        if cancellation_requested(run_root)? {
            match s.status {
                RunStatus::Running => {
                    emit(
                        run_root,
                        g,
                        s,
                        EventKind::RunFailed {
                            error: "automation cancelled by operator".into(),
                        },
                    )?;
                    clear_cancellation_request(run_root)?;
                    return Ok(());
                }
                RunStatus::Completed | RunStatus::Failed => {
                    clear_cancellation_request(run_root)?;
                    return Ok(());
                }
                RunStatus::AwaitingApproval => return Ok(()),
            }
        }
        match core::advance_loops(g, s) {
            Ok(events) => {
                for event in events {
                    emit(run_root, g, s, event)?;
                }
            }
            Err(error) => {
                emit(
                    run_root,
                    g,
                    s,
                    EventKind::RunFailed {
                        error: error.to_string(),
                    },
                )?;
                return Ok(());
            }
        }
        // Plan finalization on a clone so every skip discovered by the
        // cascade is persisted as a NodeSkipped event before it affects the
        // live state. The same keeps replay and checkpoint state aligned.
        let mut finalized = s.clone();
        finalize_if_done(g, &mut finalized);
        let newly_skipped: Vec<String> = finalized
            .nodes
            .iter()
            .filter(|(node, status)| {
                **status == NodeStatus::Skipped && s.status_or_pending(node) != NodeStatus::Skipped
            })
            .map(|(node, _)| node.clone())
            .collect();
        for node in newly_skipped {
            emit(run_root, g, s, EventKind::NodeSkipped { node })?;
        }
        write_checkpoint(run_root, s)?;
        if finalized.status != RunStatus::Running {
            if finalized.status == RunStatus::Completed {
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
            if s.status == RunStatus::AwaitingApproval {
                return Ok(());
            }
            if cancellation_requested(run_root)? {
                if s.status == RunStatus::Running {
                    emit(
                        run_root,
                        g,
                        s,
                        EventKind::RunFailed {
                            error: "automation cancelled by operator".into(),
                        },
                    )?;
                }
                if matches!(s.status, RunStatus::Completed | RunStatus::Failed) {
                    clear_cancellation_request(run_root)?;
                }
                return Ok(());
            }
            if s.status == RunStatus::Failed {
                return Ok(());
            }
        }
    }
}

fn cancellation_requested(run_root: &Path) -> crate::engine::Result<bool> {
    Ok(run_root.join("cancel.marker").exists())
}

fn clear_cancellation_request(run_root: &Path) -> crate::engine::Result<()> {
    match std::fs::remove_file(run_root.join("cancel.marker")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
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

    if crate::automation::find_node_type(&node.r#type)
        .is_some_and(|definition| !definition.supported)
    {
        emit(
            run_root,
            g,
            s,
            EventKind::NodeFailed {
                node: node.id.clone(),
                error: format!(
                    "node type `{}` is registered but not supported by the automation runtime",
                    node.r#type
                ),
            },
        )?;
        return Ok(());
    }

    // Triggers + join: pass-through completion.
    if node.r#type.ends_with("_trigger") || node.r#type == "manual_trigger" || node.r#type == "join"
    {
        let output = if node.r#type.ends_with("_trigger") || node.r#type == "manual_trigger" {
            s.registry
                .get("trigger")
                .and_then(|value| value.get("output"))
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        emit(
            run_root,
            g,
            s,
            EventKind::NodeCompleted {
                node: node.id.clone(),
                output,
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
        match eval_branch(s, &node) {
            Ok(port) => emit(
                run_root,
                g,
                s,
                EventKind::BranchTaken {
                    node: node.id.clone(),
                    port,
                },
            )?,
            Err(error) => emit(
                run_root,
                g,
                s,
                EventKind::NodeFailed {
                    node: node.id.clone(),
                    error: error.to_string(),
                },
            )?,
        }
        return Ok(());
    }

    if node.r#type == "state" {
        emit(
            run_root,
            g,
            s,
            EventKind::NodeStarted {
                node: node.id.clone(),
            },
        )?;
        match execute_state(s, &node) {
            Ok((output, update)) => {
                if let Some((op, entries)) = update {
                    emit(
                        run_root,
                        g,
                        s,
                        EventKind::StateUpdated {
                            node: node.id.clone(),
                            op,
                            entries,
                        },
                    )?;
                }
                emit(
                    run_root,
                    g,
                    s,
                    EventKind::NodeCompleted {
                        node: node.id.clone(),
                        output,
                    },
                )?;
            }
            Err(error) => emit(
                run_root,
                g,
                s,
                EventKind::NodeFailed {
                    node: node.id.clone(),
                    error: error.0,
                },
            )?,
        }
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
        Ok(step) => {
            if let Some(message) = step.notification {
                emit(
                    run_root,
                    g,
                    s,
                    EventKind::Notification {
                        node: node.id.clone(),
                        message,
                    },
                )?;
            }
            if let Some(port) = step.chosen_port {
                emit(
                    run_root,
                    g,
                    s,
                    EventKind::DecisionCompleted {
                        node: node.id.clone(),
                        output: step.output,
                        port,
                    },
                )?;
            } else {
                emit(
                    run_root,
                    g,
                    s,
                    EventKind::NodeCompleted {
                        node: node.id.clone(),
                        output: step.output,
                    },
                )?;
            }
        }
        Err(error) => {
            if error.skipped_reason().is_some() {
                emit(
                    run_root,
                    g,
                    s,
                    EventKind::NodeSkipped {
                        node: node.id.clone(),
                    },
                )?
            } else {
                emit(
                    run_root,
                    g,
                    s,
                    EventKind::NodeFailed {
                        node: node.id.clone(),
                        error: error.0,
                    },
                )?
            }
        }
    }
    Ok(())
}

/// Record loop entry as the durable event that also pulses the body in the core.
fn emit_loop_enter(
    run_root: &Path,
    g: &Graph<'_>,
    s: &mut RunState,
    loop_id: &str,
) -> crate::engine::Result<()> {
    emit(
        run_root,
        g,
        s,
        EventKind::LoopIteration {
            node: loop_id.into(),
            iteration: 0,
        },
    )
}

fn eval_branch(s: &RunState, node: &Node) -> crate::engine::Result<String> {
    let cond = node
        .fields
        .get("condition")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let path = crate::automation::condition::validate_path(cond).map_err(|message| {
        EngineError::InvalidState(format!("branch condition is invalid: {message}"))
    })?;
    let truthy = crate::automation::condition::lookup_truthy(&s.registry, &path);
    Ok(if truthy {
        "on_true".into()
    } else {
        "on_false".into()
    })
}

fn resolve_json(
    value: &serde_json::Value,
    registry: &serde_json::Value,
) -> Result<serde_json::Value, StepError> {
    match value {
        serde_json::Value::String(text) => resolve(text, registry)
            .map(serde_json::Value::String)
            .map_err(|path| StepError::new(format!("unresolved {{{{{path}}}}}"))),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| resolve_json(value, registry))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), resolve_json(value, registry)?)))
            .collect::<Result<serde_json::Map<_, _>, StepError>>()
            .map(serde_json::Value::Object),
        other => Ok(other.clone()),
    }
}

/// Execute the deterministic state node and return its durable mutation.
fn execute_state(
    s: &RunState,
    node: &Node,
) -> Result<(serde_json::Value, Option<(String, serde_json::Value)>), StepError> {
    let op = node
        .fields
        .get("op")
        .and_then(|value| value.as_str())
        .ok_or_else(|| StepError::new("state node requires an operation"))?;
    let entries = resolve_json(
        node.fields
            .get("entries")
            .unwrap_or(&serde_json::Value::Object(serde_json::Map::new())),
        &s.registry,
    )?;
    let entry_map = entries
        .as_object()
        .ok_or_else(|| StepError::new("state entries must be an object"))?;
    let storage = s
        .registry
        .get("storage")
        .and_then(|value| value.as_object())
        .ok_or_else(|| StepError::new("run registry storage must be an object"))?;

    match op {
        "get" => {
            let output = if entry_map.is_empty() {
                serde_json::Value::Object(storage.clone())
            } else {
                entry_map
                    .keys()
                    .map(|key| {
                        (
                            key.clone(),
                            storage.get(key).cloned().unwrap_or(serde_json::Value::Null),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>()
                    .into()
            };
            Ok((output, None))
        }
        "set" | "merge" | "delete" => Ok((
            if op == "delete" {
                serde_json::json!({})
            } else {
                entries.clone()
            },
            Some((op.to_string(), entries)),
        )),
        other => Err(StepError::new(format!("unknown state op: {other}"))),
    }
}

/// Interpolate the node's string fields and call the matching executor method.
async fn run_side_effect(
    g: &Graph<'_>,
    s: &RunState,
    exec: &dyn StepExecutor,
    node: &Node,
) -> Result<ExecutedStep, StepError> {
    let _ = g;
    let f = |key: &str| -> Result<String, StepError> {
        let raw = node.fields.get(key).and_then(|v| v.as_str()).unwrap_or("");
        resolve(raw, &s.registry).map_err(|p| StepError::new(format!("unresolved {{{{{p}}}}}")))
    };
    match node.r#type.as_str() {
        "task" => Ok(ExecutedStep::output(
            exec.run_agent_task(AgentTaskRequest {
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
            .0,
        )),
        "decision" => {
            let choices: Vec<String> = node
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
                    choices: choices.clone(),
                })
                .await?
                .0;
            if !choices.iter().any(|choice| choice == &port) {
                return Err(StepError::new(format!(
                    "decision chose undeclared port `{port}`"
                )));
            }
            Ok(ExecutedStep {
                output: serde_json::json!({ "chosen": port }),
                chosen_port: Some(port),
                notification: None,
            })
        }
        "shell" => Ok(ExecutedStep::output(
            exec.run_shell(ShellRequest {
                node: node.id.clone(),
                command: f("command")?,
                cwd: node
                    .fields
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
            .await?
            .0,
        )),
        "script" => Ok(ExecutedStep::output(
            exec.run_script(ScriptRequest {
                node: node.id.clone(),
                runtime: f("runtime")?,
                path: f("path")?,
            })
            .await?
            .0,
        )),
        "message_send" => Ok(ExecutedStep::output(
            exec.message_send(message_send_request(node, s)?).await?.0,
        )),
        "notify" => {
            let message = f("message")?;
            exec.notify(NotifyRequest {
                node: node.id.clone(),
                message: message.clone(),
            })
            .await?;
            Ok(ExecutedStep {
                output: serde_json::json!({}),
                chosen_port: None,
                notification: Some(message),
            })
        }
        "memory_commit" => {
            let source_node = f("source_node")?;
            let principal_template = node
                .fields
                .get("agent_id")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .unwrap_or_default();
            if principal_template != "{{trigger.output.agent_id}}" {
                return Err(StepError::new(
                    "memory_commit agent_id must be the invocation-owned {{trigger.output.agent_id}}",
                ));
            }
            let agent_id = f("agent_id")?;
            let payload = s.node_output(&source_node).cloned().ok_or_else(|| {
                StepError::new(format!(
                    "memory_commit source node `{source_node}` has no output"
                ))
            })?;
            let trigger_output = s
                .registry
                .get("trigger")
                .and_then(|value| value.get("output"));
            let trigger_string = |field: &str| {
                trigger_output
                    .and_then(|value| value.get(field))
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            };
            Ok(ExecutedStep::output(
                exec.memory_commit(MemoryCommitRequest {
                    node: node.id.clone(),
                    agent_id,
                    workspace: trigger_string("workspace"),
                    conversation_id: trigger_string("conversation_id"),
                    source_sequence: trigger_output
                        .and_then(|value| value.get("source_sequence"))
                        .and_then(|value| value.as_u64()),
                    archive_available: trigger_output
                        .and_then(|value| value.get("archive_available"))
                        .and_then(|value| value.as_bool()),
                    idempotency_key: trigger_string("idempotency_key"),
                    payload,
                })
                .await?
                .0,
            ))
        }
        other => Err(StepError::new(format!(
            "no executor for node type `{other}`"
        ))),
    }
}

struct ExecutedStep {
    output: serde_json::Value,
    chosen_port: Option<String>,
    notification: Option<String>,
}

impl ExecutedStep {
    fn output(output: serde_json::Value) -> Self {
        Self {
            output,
            chosen_port: None,
            notification: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MockExecutor;

    #[tokio::test]
    async fn start_with_id_persists_caller_supplied_run_id() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "wf".into(),
            name: "Automation".into(),
            nodes: vec![Node {
                id: "t".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };
        let exec = MockExecutor::new();

        let state = Engine::start_with_id(
            &blueprint,
            "run-xyz",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();

        assert_eq!(state.run_id, "run-xyz");
        let checkpoint = read_checkpoint(dir.path()).unwrap().unwrap();
        assert_eq!(checkpoint.run_id, "run-xyz");
    }

    fn branch_blueprint() -> Blueprint {
        let task = |id: &str| Node {
            id: id.into(),
            r#type: "task".into(),
            name: None,
            parent: None,
            fields: serde_json::json!({
                "agent": "role:worker",
                "prompt": "work"
            })
            .as_object()
            .unwrap()
            .clone(),
            position: None,
        };
        Blueprint {
            schema: 2,
            id: "branch-contract".into(),
            name: "Branch contract".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                task("agent-1"),
                Node {
                    id: "route".into(),
                    r#type: "branch".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "condition": "nodes.agent-1.output.ready"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
                task("yes"),
                task("no"),
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "agent-1".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "agent-1".into(),
                    from_port: "out".into(),
                    to: "route".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "route".into(),
                    from_port: "on_true".into(),
                    to: "yes".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "route".into(),
                    from_port: "on_false".into(),
                    to: "no".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        }
    }

    #[tokio::test]
    async fn branch_routes_boolean_truthiness_and_replays_the_same_decision() {
        for (ready, expected_port, expected_task, skipped_task) in [
            (true, "on_true", "task:yes", "no"),
            (false, "on_false", "task:no", "yes"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let blueprint = branch_blueprint();
            let exec = MockExecutor::new()
                .with_task_output("agent-1", serde_json::json!({"ready": ready}));

            let state = Engine::start_with_id(
                &blueprint,
                format!("run-{ready}"),
                serde_json::json!({}),
                dir.path(),
                &exec,
            )
            .await
            .unwrap();

            assert_eq!(state.status, RunStatus::Completed);
            assert!(exec.calls().contains(&expected_task.to_string()));
            assert_eq!(state.status_or_pending(skipped_task), NodeStatus::Skipped);
            let events = read_events(dir.path()).unwrap();
            assert!(events.iter().any(|event| {
                matches!(&event.kind, EventKind::BranchTaken { node, port } if node == "route" && port == expected_port)
            }));

            let replayed = Engine::replay(&blueprint, dir.path()).unwrap();
            assert_eq!(replayed.registry, state.registry);
            assert_eq!(replayed.status, state.status);
        }
    }

    #[tokio::test]
    async fn fresh_start_rejects_an_invalid_blueprint_before_persisting_or_executing() {
        let dir = tempfile::tempdir().unwrap();
        let mut blueprint = branch_blueprint();
        blueprint
            .nodes
            .iter_mut()
            .find(|node| node.id == "route")
            .unwrap()
            .fields
            .insert(
                "condition".into(),
                serde_json::json!("nodes.a.output.ok == true"),
            );
        let exec = MockExecutor::new();

        let error = Engine::start_with_id(
            &blueprint,
            "run-invalid-before-start",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("automation blueprint is invalid"));
        assert!(exec.calls().is_empty());
        assert!(!dir.path().join("events.jsonl").exists());
        assert!(!dir.path().join("state.json").exists());
        assert!(!dir.path().join("blueprint.json").exists());
    }

    #[tokio::test]
    async fn decision_routes_only_the_declared_choice_and_replays_routing() {
        let task = |id: &str| Node {
            id: id.into(),
            r#type: "task".into(),
            name: None,
            parent: None,
            fields: serde_json::json!({
                "agent": "role:worker",
                "prompt": "work"
            })
            .as_object()
            .unwrap()
            .clone(),
            position: None,
        };
        let blueprint = Blueprint {
            schema: 2,
            id: "decision-contract".into(),
            name: "Decision contract".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "choose".into(),
                    r#type: "decision".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "agent": "role:arbiter",
                        "prompt": "choose",
                        "choices": ["approve", "deny"]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
                task("approved"),
                task("denied"),
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "choose".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "choose".into(),
                    from_port: "approve".into(),
                    to: "approved".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "choose".into(),
                    from_port: "deny".into(),
                    to: "denied".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let exec = MockExecutor::new().with_decision("choose", "deny");

        let state = Engine::start_with_id(
            &blueprint,
            "run-decision",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();

        assert_eq!(state.status, RunStatus::Completed);
        assert!(exec.calls().contains(&"task:denied".to_string()));
        assert!(!exec.calls().contains(&"task:approved".to_string()));
        assert!(read_events(dir.path()).unwrap().iter().any(|event| {
            matches!(&event.kind, EventKind::DecisionCompleted { node, port, .. }
                if node == "choose" && port == "deny")
        }));
        assert!(state.delivered.contains_key("denied"));
        assert!(state.skipped_edges.contains(&1));

        let replayed = Engine::replay(&blueprint, dir.path()).unwrap();
        assert_eq!(replayed.registry, state.registry);
        assert_eq!(replayed.nodes, state.nodes);
        assert_eq!(replayed.delivered, state.delivered);
        assert_eq!(replayed.skipped_edges, state.skipped_edges);
    }

    #[tokio::test]
    async fn decision_completion_event_replays_routing_after_a_stale_checkpoint() {
        let blueprint = Blueprint {
            schema: 2,
            id: "decision-resume".into(),
            name: "Decision resume".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "choose".into(),
                    r#type: "decision".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "agent": "role:arbiter",
                        "prompt": "choose",
                        "choices": ["deny"]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
                Node {
                    id: "denied".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "agent": "role:worker",
                        "prompt": "deny"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "choose".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "choose".into(),
                    from_port: "deny".into(),
                    to: "denied".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let checkpoint = Engine::initialize_with_id(
            &blueprint,
            "run-decision-resume",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();
        append_event(
            dir.path(),
            &Event::at(
                checkpoint.next_seq,
                "decision-started".into(),
                EventKind::NodeStarted {
                    node: "choose".into(),
                },
            ),
        )
        .unwrap();
        append_event(
            dir.path(),
            &Event::at(
                checkpoint.next_seq + 1,
                "decision-completed".into(),
                EventKind::DecisionCompleted {
                    node: "choose".into(),
                    output: serde_json::json!({"chosen": "deny"}),
                    port: "deny".into(),
                },
            ),
        )
        .unwrap();

        let exec = MockExecutor::new();
        let state = Engine::resume(&blueprint, dir.path(), &exec).await.unwrap();

        assert_eq!(state.status, RunStatus::Completed);
        assert!(exec.calls().contains(&"task:denied".to_string()));
        assert_eq!(
            Engine::replay(&blueprint, dir.path()).unwrap().delivered,
            state.delivered
        );
    }

    #[tokio::test]
    async fn decision_rejects_an_undeclared_port_with_durable_failure() {
        let mut blueprint = branch_blueprint();
        let route = blueprint
            .nodes
            .iter_mut()
            .find(|node| node.id == "route")
            .unwrap();
        route.r#type = "decision".into();
        route.fields = serde_json::json!({
            "agent": "role:arbiter",
            "prompt": "choose",
            "choices": ["approve", "deny"]
        })
        .as_object()
        .unwrap()
        .clone();
        blueprint.edges[2].from_port = "approve".into();
        blueprint.edges[3].from_port = "deny".into();
        let dir = tempfile::tempdir().unwrap();
        let exec = MockExecutor::new().with_decision("route", "unexpected");

        let state = Engine::start_with_id(
            &blueprint,
            "run-invalid-decision",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();

        assert_eq!(state.status, RunStatus::Failed);
        assert!(read_events(dir.path()).unwrap().iter().any(|event| {
            matches!(&event.kind, EventKind::NodeFailed { node, error }
                if node == "route" && error.contains("undeclared port"))
        }));
    }

    #[tokio::test]
    async fn fresh_start_rejects_an_invalid_branch_before_persisting_or_executing() {
        let mut blueprint = branch_blueprint();
        blueprint
            .nodes
            .iter_mut()
            .find(|node| node.id == "route")
            .unwrap()
            .fields
            .insert(
                "condition".into(),
                serde_json::json!("nodes.agent-1.output.ready === true"),
            );
        let dir = tempfile::tempdir().unwrap();

        let error = Engine::start_with_id(
            &blueprint,
            "run-invalid-condition",
            serde_json::json!({}),
            dir.path(),
            &MockExecutor::new(),
        )
        .await
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("automation blueprint is invalid"));
        assert!(error.to_string().contains("operators and comparisons"));
        assert!(!dir.path().join("events.jsonl").exists());
        assert!(!dir.path().join("state.json").exists());
        assert!(!dir.path().join("blueprint.json").exists());
    }

    #[tokio::test]
    async fn fresh_start_rejects_an_invalid_loop_before_persisting_or_executing() {
        let loop_node = Node {
            id: "lp".into(),
            r#type: "loop".into(),
            name: None,
            parent: None,
            fields: serde_json::json!({
                "max_iterations": 3,
                "until": "nodes.body.output.count > 2"
            })
            .as_object()
            .unwrap()
            .clone(),
            position: None,
        };
        let blueprint = Blueprint {
            schema: 2,
            id: "loop-condition-contract".into(),
            name: "Loop condition contract".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                loop_node.clone(),
                Node {
                    id: "body".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: Some("lp".into()),
                    fields: serde_json::json!({
                        "agent": "role:worker",
                        "prompt": "work"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "lp".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "lp".into(),
                    from_port: "body".into(),
                    to: "body".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        let dir = tempfile::tempdir().unwrap();

        let error = Engine::start_with_id(
            &blueprint,
            "run-invalid-loop-condition",
            serde_json::json!({}),
            dir.path(),
            &MockExecutor::new(),
        )
        .await
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("automation blueprint is invalid"));
        assert!(error.to_string().contains("operators and comparisons"));
        assert!(!dir.path().join("events.jsonl").exists());
        assert!(!dir.path().join("state.json").exists());
        assert!(!dir.path().join("blueprint.json").exists());
    }

    #[tokio::test]
    async fn state_nodes_mutate_storage_and_replay_the_mutation() {
        let state_node = |id: &str, op: &str, entries: serde_json::Value| Node {
            id: id.into(),
            r#type: "state".into(),
            name: None,
            parent: None,
            fields: serde_json::json!({"op": op, "entries": entries})
                .as_object()
                .unwrap()
                .clone(),
            position: None,
        };
        let blueprint = Blueprint {
            schema: 2,
            id: "state-contract".into(),
            name: "State contract".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                state_node("set", "set", serde_json::json!({"branch": "main"})),
                state_node("get", "get", serde_json::json!({"branch": null})),
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "set".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "set".into(),
                    from_port: "out".into(),
                    to: "get".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let state = Engine::start_with_id(
            &blueprint,
            "run-state",
            serde_json::json!({}),
            dir.path(),
            &MockExecutor::new(),
        )
        .await
        .unwrap();

        assert_eq!(state.registry["storage"]["branch"], "main");
        assert_eq!(state.node_output("get").unwrap()["branch"], "main");
        assert!(read_events(dir.path())
            .unwrap()
            .iter()
            .any(|event| matches!(event.kind, EventKind::StateUpdated { .. })));
        let replayed = Engine::replay(&blueprint, dir.path()).unwrap();
        assert_eq!(replayed.registry, state.registry);
    }

    #[tokio::test]
    async fn notify_is_recorded_as_durable_run_evidence() {
        let blueprint = Blueprint {
            schema: 2,
            id: "notify-contract".into(),
            name: "Notify contract".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "notice".into(),
                    r#type: "notify".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({"message": "ready"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    position: None,
                },
            ],
            edges: vec![crate::automation::Edge {
                from: "trigger".into(),
                from_port: "out".into(),
                to: "notice".into(),
                to_port: "in".into(),
            }],
            body: String::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let exec = MockExecutor::new();
        let state = Engine::start_with_id(
            &blueprint,
            "run-notify",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();

        assert!(exec.calls().contains(&"notify:notice".to_string()));
        assert!(read_events(dir.path()).unwrap().iter().any(|event| {
            matches!(&event.kind, EventKind::Notification { node, message }
                if node == "notice" && message == "ready")
        }));
        assert_eq!(
            Engine::replay(&blueprint, dir.path()).unwrap().registry,
            state.registry
        );
    }

    #[tokio::test]
    async fn loop_transitions_are_durable_and_replay_to_the_live_state() {
        let blueprint = Blueprint {
            schema: 2,
            id: "loop-replay-contract".into(),
            name: "Loop replay contract".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "repeat".into(),
                    r#type: "loop".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({"max_iterations": 2})
                        .as_object()
                        .unwrap()
                        .clone(),
                    position: None,
                },
                Node {
                    id: "body".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: Some("repeat".into()),
                    fields: serde_json::json!({
                        "agent": "role:worker",
                        "prompt": "iterate"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
                Node {
                    id: "ship".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "agent": "role:worker",
                        "prompt": "ship"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "repeat".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "repeat".into(),
                    from_port: "body".into(),
                    to: "body".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "repeat".into(),
                    from_port: "done".into(),
                    to: "ship".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let exec = MockExecutor::new();
        let state = Engine::start_with_id(
            &blueprint,
            "run-loop-replay",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();

        let events = read_events(dir.path()).unwrap();
        assert!(events.iter().any(|event| {
            matches!(&event.kind, EventKind::LoopIteration { node, iteration }
                if node == "repeat" && *iteration == 0)
        }));
        assert!(events.iter().any(|event| {
            matches!(&event.kind, EventKind::LoopIteration { node, iteration }
                if node == "repeat" && *iteration == 1)
        }));
        assert!(events.iter().any(
            |event| matches!(&event.kind, EventKind::LoopCompleted { node } if node == "repeat")
        ));

        let replayed = Engine::replay(&blueprint, dir.path()).unwrap();
        assert_eq!(replayed.status, state.status);
        assert_eq!(replayed.nodes, state.nodes);
        assert_eq!(replayed.registry, state.registry);
        assert_eq!(replayed.loop_iter, state.loop_iter);
        assert_eq!(replayed.delivered, state.delivered);
        assert_eq!(replayed.skipped_edges, state.skipped_edges);
        assert_eq!(replayed.next_seq, state.next_seq);
    }

    #[tokio::test]
    async fn empty_loop_is_rejected_before_start_instead_of_leaving_the_run_active() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "empty-loop".into(),
            name: "Empty loop".into(),
            nodes: vec![Node {
                id: "repeat".into(),
                r#type: "loop".into(),
                name: None,
                parent: None,
                fields: serde_json::json!({"max_iterations": 2})
                    .as_object()
                    .unwrap()
                    .clone(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };

        let error = Engine::start_with_id(
            &blueprint,
            "run-empty-loop",
            serde_json::json!({}),
            dir.path(),
            &MockExecutor::new(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("empty_loop_body"));
        assert!(!dir.path().join("events.jsonl").exists());
        assert!(!dir.path().join("state.json").exists());
        assert!(!dir.path().join("blueprint.json").exists());
        assert!(!dir.path().join("cancel.marker").exists());
    }

    #[tokio::test]
    async fn resume_folds_events_appended_after_a_stale_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "resume-tail".into(),
            name: "Resume tail".into(),
            nodes: vec![Node {
                id: "trigger".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };
        Engine::initialize_with_id(
            &blueprint,
            "run-resume-tail",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();
        append_event(
            dir.path(),
            &Event::at(
                1,
                "tail".into(),
                EventKind::StateUpdated {
                    node: "state".into(),
                    op: "set".into(),
                    entries: serde_json::json!({"recovered": true}),
                },
            ),
        )
        .unwrap();

        let state = Engine::resume(&blueprint, dir.path(), &MockExecutor::new())
            .await
            .unwrap();

        assert_eq!(state.registry["storage"]["recovered"], true);
        assert_eq!(state.status, RunStatus::Completed);
    }

    #[tokio::test]
    async fn resume_rejects_a_checkpoint_and_event_log_with_different_run_ids() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = sequence_blueprint();
        Engine::initialize_with_id(
            &blueprint,
            "run-checkpoint-id",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();

        let mut events = read_events(dir.path()).unwrap();
        match &mut events[0].kind {
            EventKind::RunStarted { run_id, .. } => *run_id = Some("run-event-id".into()),
            other => panic!("unexpected startup event: {other:?}"),
        }
        std::fs::write(
            dir.path().join("events.jsonl"),
            events
                .iter()
                .map(|event| serde_json::to_string(event).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        append_event(
            dir.path(),
            &Event::at(1, "tail".into(), EventKind::RunCompleted),
        )
        .unwrap();

        let error = Engine::resume(&blueprint, dir.path(), &MockExecutor::new())
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("checkpoint belongs to run `run-checkpoint-id`"));
    }

    #[tokio::test]
    async fn checkpointless_recovery_preserves_the_run_id_from_run_started() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "checkpointless-recovery".into(),
            name: "Checkpointless recovery".into(),
            nodes: vec![Node {
                id: "trigger".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };
        Engine::initialize_with_id(
            &blueprint,
            "run-original-id",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();
        std::fs::remove_file(dir.path().join("state.json")).unwrap();

        let state = Engine::resume(&blueprint, dir.path(), &MockExecutor::new())
            .await
            .unwrap();

        assert_eq!(state.run_id, "run-original-id");
        assert_eq!(
            read_checkpoint(dir.path()).unwrap().unwrap().run_id,
            "run-original-id"
        );
    }

    fn sequence_blueprint() -> Blueprint {
        Blueprint {
            schema: 2,
            id: "sequence-contract".into(),
            name: "Sequence contract".into(),
            nodes: vec![Node {
                id: "trigger".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        }
    }

    #[test]
    fn replay_launch_failure_accepts_a_terminal_artifact_without_a_blueprint() {
        let dir = tempfile::tempdir().unwrap();
        let message = "could not resolve blueprint path";
        let mut state = RunState::new("run-launch-failure", "missing-automation");
        state.status = RunStatus::Failed;
        state.failure = Some(message.into());
        state.next_seq = 1;
        write_checkpoint(dir.path(), &state).unwrap();
        append_event(
            dir.path(),
            &Event::new(
                0,
                EventKind::RunFailed {
                    error: message.into(),
                },
            ),
        )
        .unwrap();

        let replayed = Engine::replay_launch_failure("missing-automation", dir.path()).unwrap();

        assert_eq!(replayed.status, RunStatus::Failed);
        assert_eq!(replayed.failure.as_deref(), Some(message));
        assert_eq!(replayed.next_seq, 1);
    }

    #[tokio::test]
    async fn replay_and_resume_reject_a_sequence_gap() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = sequence_blueprint();
        Engine::initialize_with_id(
            &blueprint,
            "run-sequence-gap",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();
        append_event(
            dir.path(),
            &Event::at(
                2,
                "gap".into(),
                EventKind::NodeCompleted {
                    node: "trigger".into(),
                    output: serde_json::json!({}),
                },
            ),
        )
        .unwrap();

        assert!(Engine::replay(&blueprint, dir.path())
            .unwrap_err()
            .to_string()
            .contains("expected 1, got 2"));
        assert!(Engine::resume(&blueprint, dir.path(), &MockExecutor::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("expected 1, got 2"));
    }

    #[tokio::test]
    async fn replay_and_resume_reject_out_of_order_events() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = sequence_blueprint();
        Engine::initialize_with_id(
            &blueprint,
            "run-sequence-order",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();
        let first = read_events(dir.path()).unwrap().remove(0);
        let second = Event::at(
            1,
            "second".into(),
            EventKind::NodeCompleted {
                node: "trigger".into(),
                output: serde_json::json!({}),
            },
        );
        let third = Event::at(2, "third".into(), EventKind::RunCompleted);
        std::fs::write(
            dir.path().join("events.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                serde_json::to_string(&first).unwrap(),
                serde_json::to_string(&third).unwrap(),
                serde_json::to_string(&second).unwrap()
            ),
        )
        .unwrap();

        assert!(Engine::replay(&blueprint, dir.path())
            .unwrap_err()
            .to_string()
            .contains("expected 1, got 2"));
        assert!(Engine::resume(&blueprint, dir.path(), &MockExecutor::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("expected 1, got 2"));
    }

    #[tokio::test]
    async fn fresh_start_rejects_an_unsupported_sub_automation_before_persisting() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "sub-automation-contract".into(),
            name: "Sub-automation contract".into(),
            nodes: vec![Node {
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
            edges: vec![],
            body: String::new(),
        };

        let error = Engine::start_with_id(
            &blueprint,
            "run-sub-automation",
            serde_json::json!({}),
            dir.path(),
            &MockExecutor::new(),
        )
        .await
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("node type `sub_automation` is registered but not supported"));
        assert!(!dir.path().join("events.jsonl").exists());
        assert!(!dir.path().join("state.json").exists());
        assert!(!dir.path().join("blueprint.json").exists());
    }

    #[tokio::test]
    async fn cancellation_marker_is_consumed_at_the_next_driver_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "cancel-contract".into(),
            name: "Cancel contract".into(),
            nodes: vec![Node {
                id: "trigger".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };
        let state =
            Engine::initialize_with_id(&blueprint, "run-cancel", serde_json::json!({}), dir.path())
                .unwrap();
        std::fs::write(dir.path().join("cancel.marker"), "cancelled").unwrap();

        let state = Engine::drive_from_state(&blueprint, state, dir.path(), &MockExecutor::new())
            .await
            .unwrap();

        assert_eq!(state.status, RunStatus::Failed);
        assert_eq!(
            state.failure.as_deref(),
            Some("automation cancelled by operator")
        );
        assert!(read_events(dir.path()).unwrap().iter().any(|event| {
            matches!(&event.kind, EventKind::RunFailed { error } if error == "automation cancelled by operator")
        }));
        assert!(!dir.path().join("cancel.marker").exists());
    }

    #[tokio::test]
    async fn cancellation_marker_survives_event_persistence_failure() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "cancel-persistence".into(),
            name: "Cancel persistence".into(),
            nodes: vec![Node {
                id: "trigger".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };
        let state = Engine::initialize_with_id(
            &blueprint,
            "run-cancel-persistence",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();
        std::fs::remove_file(dir.path().join("events.jsonl")).unwrap();
        std::fs::create_dir(dir.path().join("events.jsonl")).unwrap();
        std::fs::write(dir.path().join("cancel.marker"), "cancelled").unwrap();

        let result =
            Engine::drive_from_state(&blueprint, state, dir.path(), &MockExecutor::new()).await;

        assert!(result.is_err());
        assert!(dir.path().join("cancel.marker").exists());
    }

    #[tokio::test]
    async fn terminal_runs_clear_stale_cancellation_markers_without_emitting_again() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "cancel-terminal".into(),
            name: "Cancel terminal".into(),
            nodes: vec![Node {
                id: "trigger".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };
        let mut state = Engine::initialize_with_id(
            &blueprint,
            "run-cancel-terminal",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();
        state.status = RunStatus::Failed;
        state.failure = Some("original failure".into());
        write_checkpoint(dir.path(), &state).unwrap();
        std::fs::write(dir.path().join("cancel.marker"), "cancelled").unwrap();

        let resumed = Engine::resume(&blueprint, dir.path(), &MockExecutor::new())
            .await
            .unwrap();

        assert_eq!(resumed.status, RunStatus::Failed);
        assert_eq!(resumed.failure.as_deref(), Some("original failure"));
        assert!(!dir.path().join("cancel.marker").exists());
        assert_eq!(read_events(dir.path()).unwrap().len(), 1);
    }

    fn approval_blueprint() -> Blueprint {
        Blueprint {
            schema: 2,
            id: "wf".into(),
            name: "Automation".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "gate".into(),
                    r#type: "approval".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "task".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "agent": "role:worker",
                        "prompt": "work"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "gate".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "gate".into(),
                    from_port: "out".into(),
                    to: "task".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        }
    }

    #[tokio::test]
    async fn cancel_approval_parked_run_persists_terminal_failure_and_cleans_marker() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = approval_blueprint();

        let parked = Engine::start_with_id(
            &blueprint,
            "run-cancel-approval",
            serde_json::json!({}),
            dir.path(),
            &MockExecutor::new(),
        )
        .await
        .unwrap();
        assert_eq!(parked.status, RunStatus::AwaitingApproval);
        std::fs::write(dir.path().join("cancel.marker"), "cancelled").unwrap();

        let cancelled = Engine::cancel(&blueprint, dir.path()).unwrap();

        assert_eq!(cancelled.status, RunStatus::Failed);
        assert_eq!(
            cancelled.failure.as_deref(),
            Some("automation cancelled by operator")
        );
        assert_eq!(
            read_checkpoint(dir.path()).unwrap().unwrap().status,
            RunStatus::Failed
        );
        assert!(read_events(dir.path()).unwrap().iter().any(|event| {
            matches!(&event.kind, EventKind::RunFailed { error }
                if error == "automation cancelled by operator")
        }));
        assert!(!dir.path().join("cancel.marker").exists());
    }

    #[tokio::test]
    async fn record_approval_granted_persists_running_state_before_continuation() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = approval_blueprint();
        let exec = MockExecutor::new();

        let parked = Engine::start_with_id(
            &blueprint,
            "run-xyz",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();
        assert_eq!(parked.status, RunStatus::AwaitingApproval);

        let accepted =
            Engine::record_approval_granted(&blueprint, dir.path(), "gate", "user", None).unwrap();

        assert_eq!(accepted.status, RunStatus::Running);
        assert_eq!(
            read_checkpoint(dir.path()).unwrap().unwrap().status,
            RunStatus::Running
        );
        assert!(!exec.calls().contains(&"task:work".to_string()));
    }

    #[tokio::test]
    async fn approval_decision_must_target_the_parked_approval_node() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = approval_blueprint();

        let parked = Engine::start_with_id(
            &blueprint,
            "run-approval-target",
            serde_json::json!({}),
            dir.path(),
            &MockExecutor::new(),
        )
        .await
        .unwrap();
        assert_eq!(parked.status, RunStatus::AwaitingApproval);

        let error = Engine::record_approval_granted(&blueprint, dir.path(), "task", "user", None)
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("approval decision targets non-approval node `task`"));
        assert_eq!(
            read_checkpoint(dir.path()).unwrap().unwrap().status,
            RunStatus::AwaitingApproval
        );
    }

    #[tokio::test]
    async fn resume_rejects_a_blueprint_changed_after_run_start() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = sequence_blueprint();
        Engine::initialize_with_id(
            &blueprint,
            "run-blueprint-immutable",
            serde_json::json!({}),
            dir.path(),
        )
        .unwrap();

        let mut changed = blueprint.clone();
        changed.name = "Edited after start".into();
        let error = Engine::resume(&changed, dir.path(), &MockExecutor::new())
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("automation blueprint changed after run start"));
        assert_eq!(
            read_checkpoint(dir.path()).unwrap().unwrap().status,
            RunStatus::Running
        );
    }

    #[tokio::test]
    async fn approval_cancellation_uses_durable_state_without_blueprint_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = approval_blueprint();
        let parked = Engine::start_with_id(
            &blueprint,
            "run-cancel-without-blueprint",
            serde_json::json!({}),
            dir.path(),
            &MockExecutor::new(),
        )
        .await
        .unwrap();
        assert_eq!(parked.status, RunStatus::AwaitingApproval);
        std::fs::remove_file(dir.path().join("blueprint.json")).unwrap();

        let cancelled = Engine::cancel_awaiting(dir.path()).unwrap();

        assert_eq!(cancelled.status, RunStatus::Failed);
        assert_eq!(
            cancelled.failure.as_deref(),
            Some("automation cancelled by operator")
        );
        assert!(read_events(dir.path()).unwrap().iter().any(|event| {
            matches!(&event.kind, EventKind::RunFailed { error }
                if error == "automation cancelled by operator")
        }));
    }

    #[tokio::test]
    async fn approval_transitions_reject_another_in_progress_decision() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = approval_blueprint();
        let exec = MockExecutor::new();
        Engine::start_with_id(
            &blueprint,
            "run-xyz",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();
        let _approval_decision =
            crate::automation_approval_lock::acquire_approval_decision_guard(dir.path()).unwrap();

        assert!(matches!(
            Engine::record_approval_granted(&blueprint, dir.path(), "gate", "user", None),
            Err(EngineError::ApprovalDecisionInProgress)
        ));
        assert!(matches!(
            Engine::reject_approval(&blueprint, dir.path(), "gate", "user", None).await,
            Err(EngineError::ApprovalDecisionInProgress)
        ));
    }

    #[test]
    fn initialize_with_id_persists_started_checkpoint_before_driving() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "wf".into(),
            name: "Automation".into(),
            nodes: vec![Node {
                id: "t".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };

        let state = Engine::initialize_with_id(
            &blueprint,
            "run-xyz",
            serde_json::json!({"source":"manual"}),
            dir.path(),
        )
        .unwrap();

        assert_eq!(state.run_id, "run-xyz");
        let checkpoint = read_checkpoint(dir.path()).unwrap().unwrap();
        assert_eq!(checkpoint.run_id, "run-xyz");
        assert_eq!(checkpoint.next_seq, 1);
        let events = read_events(dir.path()).unwrap();
        assert!(matches!(
            events.first().map(|event| &event.kind),
            Some(EventKind::RunStarted { .. })
        ));
    }

    #[tokio::test]
    async fn trigger_node_outputs_runtime_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "wf".into(),
            name: "Automation".into(),
            nodes: vec![Node {
                id: "trigger".into(),
                r#type: "manual_trigger".into(),
                name: None,
                parent: None,
                fields: serde_json::Map::new(),
                position: None,
            }],
            edges: vec![],
            body: String::new(),
        };
        let exec = MockExecutor::new();

        let state = Engine::start_with_id(
            &blueprint,
            "run-xyz",
            serde_json::json!({"source":"manual"}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();

        let global_timestamp = state.registry["trigger"]["output"]["timestamp"]
            .as_str()
            .expect("global trigger timestamp");
        let node_timestamp = state.registry["nodes"]["trigger"]["output"]["timestamp"]
            .as_str()
            .expect("trigger node timestamp");

        assert_eq!(node_timestamp, global_timestamp);
        assert_eq!(
            state.registry["nodes"]["trigger"]["output"]["source"],
            "manual"
        );
    }

    #[tokio::test]
    async fn skipped_step_emits_node_skipped_instead_of_failing_run() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "wf".into(),
            name: "Automation".into(),
            nodes: vec![
                Node {
                    id: "start".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "task".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "agent": "role:worker",
                        "prompt": "work"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
            ],
            edges: vec![crate::automation::Edge {
                from: "start".into(),
                from_port: "out".into(),
                to: "task".into(),
                to_port: "in".into(),
            }],
            body: String::new(),
        };
        let exec = MockExecutor::new().with_skipped("task", "busy");

        let state = Engine::start_with_id(
            &blueprint,
            "run-xyz",
            serde_json::json!({}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();
        let events = read_events(dir.path()).unwrap();

        assert_eq!(state.status, RunStatus::Completed);
        assert_eq!(state.nodes["task"], NodeStatus::Skipped);
        assert!(events.iter().any(|event| matches!(
            event.kind,
            EventKind::NodeSkipped { ref node } if node == "task"
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event.kind, EventKind::NodeFailed { .. })));
    }

    #[tokio::test]
    async fn memory_commit_receives_the_named_upstream_output() {
        let dir = tempfile::tempdir().unwrap();
        let payload = serde_json::json!({
            "agent_id": "agent-a",
            "idempotency_key": "run-1:conv-1:4",
            "operations": []
        });
        let blueprint = Blueprint {
            schema: 2,
            id: "memory-automation".into(),
            name: "Memory automation".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "extract".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({"agent":"role:curator","prompt":"extract"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    position: None,
                },
                Node {
                    id: "commit".into(),
                    r#type: "memory_commit".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({"source_node":"extract", "agent_id":"{{trigger.output.agent_id}}"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    position: None,
                },
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "extract".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "extract".into(),
                    from_port: "out".into(),
                    to: "commit".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        let exec = MockExecutor::new().with_task_output("extract", payload.clone());

        let state = Engine::start_with_id(
            &blueprint,
            "run-memory",
            serde_json::json!({"agent_id":"agent-a"}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();

        assert_eq!(state.status, RunStatus::Completed);
        assert_eq!(state.node_output("commit"), Some(&payload));
        assert_eq!(exec.calls(), vec!["task:extract", "memory_commit:commit"]);
    }

    #[tokio::test]
    async fn memory_commit_rejects_a_model_output_principal() {
        let dir = tempfile::tempdir().unwrap();
        let blueprint = Blueprint {
            schema: 2,
            id: "spoofed-memory-automation".into(),
            name: "Spoofed memory automation".into(),
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    r#type: "manual_trigger".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::Map::new(),
                    position: None,
                },
                Node {
                    id: "extract".into(),
                    r#type: "task".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({"agent":"role:curator","prompt":"extract"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    position: None,
                },
                Node {
                    id: "commit".into(),
                    r#type: "memory_commit".into(),
                    name: None,
                    parent: None,
                    fields: serde_json::json!({
                        "source_node":"extract",
                        "agent_id":"{{nodes.extract.output.agent_id}}"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    position: None,
                },
            ],
            edges: vec![
                crate::automation::Edge {
                    from: "trigger".into(),
                    from_port: "out".into(),
                    to: "extract".into(),
                    to_port: "in".into(),
                },
                crate::automation::Edge {
                    from: "extract".into(),
                    from_port: "out".into(),
                    to: "commit".into(),
                    to_port: "in".into(),
                },
            ],
            body: String::new(),
        };
        let exec = MockExecutor::new().with_task_output(
            "extract",
            serde_json::json!({"agent_id":"agent-b", "operations": []}),
        );

        let state = Engine::start_with_id(
            &blueprint,
            "run-spoofed-memory",
            serde_json::json!({"agent_id":"agent-a"}),
            dir.path(),
            &exec,
        )
        .await
        .unwrap();

        assert_eq!(state.status, RunStatus::Failed);
        assert_eq!(exec.calls(), vec!["task:extract"]);
        assert!(state
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("invocation-owned")));
    }
}
