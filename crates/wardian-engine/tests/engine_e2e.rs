use wardian_engine::driver::Engine;
use wardian_engine::executor::MockExecutor;
use wardian_engine::state::RunStatus;
use wardian_workflow::parse_str;

const LINEAR: &str = r#"---
schema: 2
id: linear
name: Linear
nodes:
  - id: t
    type: manual_trigger
  - id: plan
    type: task
    fields: { agent: role:planner, prompt: "plan {{trigger.output.goal}}" }
  - id: build
    type: shell
    fields: { command: "echo build" }
---
# Linear
"#;

#[tokio::test]
async fn runs_a_linear_workflow_to_completion() {
    let dir = tempfile::tempdir().unwrap();
    let bp = parse_str(LINEAR).unwrap();
    let exec = MockExecutor::new().with_task_output("plan", serde_json::json!({"ok": true}));
    let state = Engine::start(&bp, serde_json::json!({"goal": "ship"}), dir.path(), &exec)
        .await
        .unwrap();
    assert_eq!(state.status, RunStatus::Completed);
    assert_eq!(state.node_output("plan").unwrap()["ok"], true);
    // executor saw the agent task and the shell, prompt was interpolated
    assert!(exec.calls().contains(&"task:plan".to_string()));
    assert!(exec.calls().contains(&"shell:build".to_string()));
    // events were persisted
    let events = wardian_engine::store::read_events(dir.path()).unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e.kind, wardian_engine::event::EventKind::RunCompleted)));
}

const GATED: &str = r#"---
schema: 2
id: gated
name: Gated
nodes:
  - id: t
    type: manual_trigger
  - id: gate
    type: approval
    fields: { prompt: "Ship?" }
  - id: ship
    type: task
    fields: { agent: role:x, prompt: "ship it" }
---
# Gated
"#;

#[tokio::test]
async fn parks_on_approval_then_resumes_on_grant() {
    let dir = tempfile::tempdir().unwrap();
    let bp = parse_str(GATED).unwrap();
    let exec = MockExecutor::new();
    let parked = Engine::start(&bp, serde_json::json!({}), dir.path(), &exec)
        .await
        .unwrap();
    assert_eq!(parked.status, RunStatus::AwaitingApproval);
    assert!(!exec.calls().contains(&"task:ship".to_string())); // ship didn't run yet

    let done = Engine::grant_approval(&bp, dir.path(), "gate", "tan", None, &exec)
        .await
        .unwrap();
    assert_eq!(done.status, RunStatus::Completed);
    assert!(exec.calls().contains(&"task:ship".to_string()));
}

#[tokio::test]
async fn replay_reconstructs_the_same_terminal_state() {
    let dir = tempfile::tempdir().unwrap();
    let bp = parse_str(LINEAR).unwrap();
    let exec = MockExecutor::new().with_task_output("plan", serde_json::json!({"ok": true}));
    let live = Engine::start(&bp, serde_json::json!({"goal": "ship"}), dir.path(), &exec)
        .await
        .unwrap();

    let replayed = Engine::replay(&bp, dir.path()).unwrap();
    assert_eq!(replayed.status, live.status);
    assert_eq!(replayed.nodes, live.nodes);
    assert_eq!(replayed.node_output("plan"), live.node_output("plan"));
}

#[tokio::test]
async fn resume_from_checkpoint_finishes_a_parked_run() {
    let dir = tempfile::tempdir().unwrap();
    let bp = parse_str(GATED).unwrap();
    let exec = MockExecutor::new();
    // Park, then simulate a process restart: resume() on the parked run is a no-op
    // (still awaiting), and grant drives it to completion using only on-disk state.
    Engine::start(&bp, serde_json::json!({}), dir.path(), &exec)
        .await
        .unwrap();
    let still_parked = Engine::resume(&bp, dir.path(), &exec).await.unwrap();
    assert_eq!(still_parked.status, RunStatus::AwaitingApproval);
    let done = Engine::grant_approval(&bp, dir.path(), "gate", "tan", None, &exec)
        .await
        .unwrap();
    assert_eq!(done.status, RunStatus::Completed);
}
