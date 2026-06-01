use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use wardian_core::engine::{
    driver::new_run_id,
    event::EventKind,
    store::{read_checkpoint, read_events},
    RunStatus,
};

const INVOKER_BLUEPRINT: &str = r#"---
schema: 2
id: invoker
name: Invoker
nodes:
  - id: trigger-1
    type: manual_trigger
    fields:
      input_schema: '{"type":"object","properties":{"symbol":{"type":"string"}}}'
  - id: analyze
    type: task
    fields:
      agent: role:analyst
      prompt: Analyze {{trigger.output.symbol}}
edges:
  - from: trigger-1
    to: analyze
---

# Invoker
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

#[tokio::test(flavor = "current_thread")]
async fn input_interpolates_and_role_binding_selects_provider() {
    let home = tempfile::tempdir().unwrap();
    let workflows_dir = home.path().join("library").join("workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    let blueprint_path = workflows_dir.join("invoker.md");
    std::fs::write(&blueprint_path, INVOKER_BLUEPRINT).unwrap();

    let _env = EnvGuard::set(home.path(), &mock_script_path());

    let blueprint = wardian_core::workflow::parse_file(&blueprint_path).unwrap();
    let report = wardian_core::workflow::validate(&blueprint);
    assert!(report.is_valid(), "diagnostics: {:?}", report.diagnostics);

    let run_id = new_run_id();
    let run_root = wardian_core::paths::workflow_run_dir(&blueprint.id, &run_id).unwrap();
    wardian_app_lib::workflow::runs::drive_new_run(
        blueprint,
        run_id,
        run_root.clone(),
        home.path().to_path_buf(),
        "codex".into(),
        serde_json::json!({ "symbol": "SPY" }),
        HashMap::from([("analyst".to_string(), "mock".to_string())]),
    )
    .await
    .unwrap();

    let state = read_checkpoint(&run_root).unwrap().unwrap();
    assert_eq!(state.status, RunStatus::Completed);
    assert!(state.node_output("analyze").is_some());
    assert_eq!(state.registry["trigger"]["output"]["symbol"], "SPY");

    let events = read_events(&run_root).unwrap();
    let started = events.iter().find_map(|event| match &event.kind {
        EventKind::RunStarted { trigger, .. } => Some(trigger),
        _ => None,
    });
    assert_eq!(started, Some(&serde_json::json!({ "symbol": "SPY" })));
}
