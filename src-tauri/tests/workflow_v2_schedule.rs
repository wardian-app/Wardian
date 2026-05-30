//! A due schedule fires a real v2 run via the mock provider; a paused one does not.

use std::sync::{Mutex, MutexGuard, OnceLock};
use wardian_core::engine::{
    driver::new_run_id,
    store::read_checkpoint,
    RunStatus,
};

const SCHEDULED_BLUEPRINT: &str = r#"---
schema: 2
id: sched-test
name: Scheduled Test
nodes:
  - id: trigger
    type: manual_trigger
    fields:
      input_schema: '{"type":"object","properties":{"symbol":{"type":"string"}}}'
  - id: analyze
    type: task
    fields:
      agent: role:analyst
      prompt: Scheduled analysis for {{trigger.output.symbol}}
edges:
  - from: trigger
    to: analyze
---

# Scheduled Test
"#;

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    previous_home: Option<std::ffi::OsString>,
    previous_session_id: Option<std::ffi::OsString>,
    previous_mock_scenario: Option<std::ffi::OsString>,
    previous_mock_delay: Option<std::ffi::OsString>,
    previous_mock_script: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(home: &std::path::Path, mock_script: &std::path::Path) -> Self {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let guard = Self {
            _lock: lock,
            previous_home: std::env::var_os("WARDIAN_HOME"),
            previous_session_id: std::env::var_os("WARDIAN_SESSION_ID"),
            previous_mock_scenario: std::env::var_os("WARDIAN_MOCK_SCENARIO"),
            previous_mock_delay: std::env::var_os("WARDIAN_MOCK_DELAY_MS"),
            previous_mock_script: std::env::var_os("WARDIAN_MOCK_SCRIPT"),
        };

        std::env::set_var("WARDIAN_HOME", home);
        std::env::remove_var("WARDIAN_SESSION_ID");
        std::env::set_var("WARDIAN_MOCK_SCENARIO", "basic");
        std::env::set_var("WARDIAN_MOCK_DELAY_MS", "0");
        std::env::set_var("WARDIAN_MOCK_SCRIPT", mock_script);

        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        restore_env("WARDIAN_HOME", self.previous_home.take());
        restore_env("WARDIAN_SESSION_ID", self.previous_session_id.take());
        restore_env("WARDIAN_MOCK_SCENARIO", self.previous_mock_scenario.take());
        restore_env("WARDIAN_MOCK_DELAY_MS", self.previous_mock_delay.take());
        restore_env("WARDIAN_MOCK_SCRIPT", self.previous_mock_script.take());
    }
}

fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

fn mock_script_path() -> std::path::PathBuf {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("mock-agent.cjs");
    assert!(script.exists(), "mock-agent.cjs not found at {:?}", script);
    script
}

fn seed_blueprint(home: &std::path::Path) -> std::path::PathBuf {
    let workflows_dir = home.join("library").join("workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    let path = workflows_dir.join("sched-test.md");
    std::fs::write(&path, SCHEDULED_BLUEPRINT).unwrap();
    path
}

fn schedule(paused: bool, due_at: u64) -> wardian_core::models::WorkflowSchedule {
    wardian_core::models::WorkflowSchedule {
        id: "s1".into(),
        blueprint_id: "sched-test".into(),
        name: "Sched".into(),
        provider: Some("mock".into()),
        workspace: None,
        input: serde_json::json!({ "symbol": "SPY" }),
        bindings: Default::default(),
        schedule: wardian_core::models::ScheduleDefinition {
            schedule_type: "interval".into(),
            interval_minutes: Some(60),
            active: true,
            ..Default::default()
        },
        next_run_epoch_ms: Some(due_at),
        paused_remaining_ms: None,
        is_paused: paused,
        last_run_status: None,
        last_run_error: None,
        last_run_epoch_ms: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn due_schedule_fires_and_writes_a_run() {
    let home = tempfile::tempdir().unwrap();
    let blueprint_path = seed_blueprint(home.path());
    let _env = EnvGuard::set(home.path(), &mock_script_path());

    let blueprint = wardian_core::workflow::parse_file(&blueprint_path).unwrap();
    let report = wardian_core::workflow::validate(&blueprint);
    assert!(report.is_valid(), "diagnostics: {:?}", report.diagnostics);

    let now = chrono::Utc::now().timestamp_millis() as u64;
    let mut schedules = vec![schedule(false, now.saturating_sub(1_000))];
    let fires = wardian_core::schedule::plan_tick(&mut schedules, now);
    assert_eq!(fires.len(), 1, "schedule should be due");
    assert_eq!(fires[0].blueprint_id, "sched-test");
    assert_eq!(fires[0].provider.as_deref(), Some("mock"));

    let run_id = new_run_id();
    let run_root = wardian_core::paths::workflow_run_dir(&blueprint.id, &run_id).unwrap();
    wardian_app_lib::workflow_v2::runs::drive_new_run(
        blueprint,
        run_id,
        run_root.clone(),
        home.path().to_path_buf(),
        fires[0].provider.clone().unwrap_or_else(|| "codex".to_string()),
        fires[0].input.clone(),
        fires[0].bindings.clone(),
    )
    .await
    .unwrap();

    let state = read_checkpoint(&run_root).unwrap().unwrap();
    assert_eq!(state.status, RunStatus::Completed);
    assert_eq!(state.registry["trigger"]["output"]["symbol"], "SPY");
    assert!(state.node_output("analyze").is_some());
}

#[test]
fn paused_schedule_is_not_due() {
    let now = chrono::Utc::now().timestamp_millis() as u64;
    let mut schedules = vec![schedule(true, now.saturating_sub(1_000))];
    assert!(wardian_core::schedule::plan_tick(&mut schedules, now).is_empty());
}
