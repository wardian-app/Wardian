use std::sync::{Mutex, MutexGuard, OnceLock};
use wardian_core::engine::{driver::new_run_id, Engine, RunStatus};

const DEMO_BLUEPRINT: &str = r#"---
schema: 2
id: demo
name: Demo
nodes:
  - id: trigger-1
    type: manual_trigger
  - id: plan
    type: task
    fields:
      agent: role:Coder
      prompt: Return a tiny plan
edges:
  - from: trigger-1
    to: plan
---

# Demo
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
async fn mock_provider_drives_workflow_run_to_completion() {
    let home = tempfile::tempdir().unwrap();
    let workflows_dir = home.path().join("library").join("workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    let demo_path = workflows_dir.join("demo.md");
    std::fs::write(&demo_path, DEMO_BLUEPRINT).unwrap();

    let _env = EnvGuard::set(home.path(), &mock_script_path());

    let blueprint = wardian_core::workflow::parse_file(&demo_path).unwrap();
    let report = wardian_core::workflow::validate(&blueprint);
    assert!(report.is_valid(), "diagnostics: {:?}", report.diagnostics);

    let run_id = new_run_id();
    let run_root = wardian_core::paths::workflow_run_dir(&blueprint.id, &run_id).unwrap();
    let exec = wardian_app_lib::workflow::runs::live_executor(
        home.path().to_path_buf(),
        "mock".into(),
        std::collections::HashMap::new(),
    );

    let state = Engine::start_with_id(
        &blueprint,
        run_id.clone(),
        serde_json::json!({}),
        &run_root,
        &exec,
    )
    .await
    .unwrap();

    assert_eq!(state.status, RunStatus::Completed);
    assert!(run_root.join("events.jsonl").is_file());
    assert!(run_root.join("state.json").is_file());
    assert!(state.node_output("plan").is_some());
}
