use crate::state::AppState;
use crate::workflow::{
    resolve::AgentBinding,
    runner::{HeadlessAgentRunner, TauriLiveAgentRunner},
    LiveStepExecutor,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wardian_core::engine::store::read_checkpoint;
use wardian_core::engine::{Engine, RunStatus};
use wardian_core::models::{
    AgentConfig, InvocationKind, WorkflowAssignments, WorkflowRoleAssignment,
};
use wardian_core::workflow::Blueprint;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowRunInvocation {
    pub schema: u8,
    pub provider: String,
    pub workspace: String,
    #[serde(default)]
    pub bindings: HashMap<String, String>,
    #[serde(default)]
    pub assignments: WorkflowAssignments,
}

/// Scan `<runs_dir>/<id>/<run>/state.json` for runs still marked Running.
/// Returns `(blueprint_id, run_id)` pairs for resume affordances.
pub fn scan_interrupted_runs(runs_dir: &Path) -> Vec<(String, String)> {
    let mut interrupted = Vec::new();
    let Ok(blueprints) = std::fs::read_dir(runs_dir) else {
        return interrupted;
    };

    for blueprint in blueprints.flatten().filter(|entry| entry.path().is_dir()) {
        let Ok(runs) = std::fs::read_dir(blueprint.path()) else {
            continue;
        };

        for run in runs.flatten().filter(|entry| entry.path().is_dir()) {
            if let Ok(Some(state)) = read_checkpoint(&run.path()) {
                if state.status == RunStatus::Running {
                    interrupted.push((state.blueprint_id, state.run_id));
                }
            }
        }
    }

    interrupted
}

/// Build the live executor for a run in `workspace` with `default_provider`.
pub fn live_executor(
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
) -> LiveStepExecutor {
    live_executor_with_catalog(workspace, default_provider, bindings, HashMap::new())
}

pub fn live_executor_with_catalog(
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
    agent_catalog: HashMap<String, AgentBinding>,
) -> LiveStepExecutor {
    LiveStepExecutor::new(
        Arc::new(HeadlessAgentRunner),
        workspace,
        default_provider,
        bindings,
        agent_catalog,
    )
}

pub fn live_executor_with_catalog_and_assignments(
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
    assignments: WorkflowAssignments,
    agent_catalog: HashMap<String, AgentBinding>,
) -> LiveStepExecutor {
    LiveStepExecutor::new_with_assignments_and_live_runner(
        Arc::new(HeadlessAgentRunner),
        None,
        workspace,
        default_provider,
        bindings,
        assignments,
        agent_catalog,
    )
}

pub fn live_executor_with_catalog_and_app(
    app: tauri::AppHandle,
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
    agent_catalog: HashMap<String, AgentBinding>,
) -> LiveStepExecutor {
    LiveStepExecutor::new_with_live_runner(
        Arc::new(HeadlessAgentRunner),
        Some(Arc::new(TauriLiveAgentRunner::new(app))),
        workspace,
        default_provider,
        bindings,
        agent_catalog,
    )
}

pub fn live_executor_with_catalog_assignments_and_app(
    app: tauri::AppHandle,
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
    assignments: WorkflowAssignments,
    agent_catalog: HashMap<String, AgentBinding>,
) -> LiveStepExecutor {
    LiveStepExecutor::new_with_assignments_and_live_runner(
        Arc::new(HeadlessAgentRunner),
        Some(Arc::new(TauriLiveAgentRunner::new(app))),
        workspace,
        default_provider,
        bindings,
        assignments,
        agent_catalog,
    )
}

fn invocation_path(run_root: &Path) -> PathBuf {
    run_root.join("invocation.json")
}

pub fn write_run_invocation(
    run_root: &Path,
    provider: &str,
    workspace: &Path,
    bindings: &HashMap<String, String>,
    assignments: &WorkflowAssignments,
) -> Result<(), String> {
    std::fs::create_dir_all(run_root)
        .map_err(|error| format!("failed to create run directory: {error}"))?;
    let invocation = WorkflowRunInvocation {
        schema: 1,
        provider: provider.to_string(),
        workspace: workspace.to_string_lossy().to_string(),
        bindings: bindings.clone(),
        assignments: assignments.clone(),
    };
    let body = serde_json::to_string_pretty(&invocation)
        .map_err(|error| format!("failed to serialize workflow invocation: {error}"))?;
    std::fs::write(invocation_path(run_root), body)
        .map_err(|error| format!("failed to write workflow invocation: {error}"))
}

pub fn read_run_invocation(run_root: &Path) -> Result<Option<WorkflowRunInvocation>, String> {
    let path = invocation_path(run_root);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read workflow invocation: {error}"))?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| format!("failed to parse workflow invocation: {error}"))
}

pub async fn agent_catalog_from_state(
    state: &AppState,
    bindings: &HashMap<String, String>,
    workspace: &Path,
    default_provider: &str,
) -> HashMap<String, AgentBinding> {
    agent_catalog_from_state_with_assignments(
        state,
        bindings,
        &WorkflowAssignments::new(),
        workspace,
        default_provider,
    )
    .await
}

pub async fn agent_catalog_from_state_with_assignments(
    state: &AppState,
    bindings: &HashMap<String, String>,
    assignments: &WorkflowAssignments,
    workspace: &Path,
    default_provider: &str,
) -> HashMap<String, AgentBinding> {
    let mut catalog = HashMap::new();
    let live_senders = state
        .input_senders
        .read()
        .map(|senders| {
            senders
                .keys()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    {
        let agents = state.agents.lock().await;
        for agent in agents.values() {
            if let Ok(config) = agent.config.lock() {
                let current_status = agent
                    .current_status
                    .lock()
                    .map(|status| status.clone())
                    .unwrap_or_default();
                let normalized_status = wardian_core::identity::normalize_status(&current_status);
                let is_live = !config.is_off && live_senders.contains(&config.session_id);
                let is_input_ready = is_live
                    && !matches!(
                        normalized_status.as_str(),
                        "processing" | "action_required" | "headless" | "off" | "error"
                    );
                if let Some(binding) = agent_binding_from_config(
                    &config,
                    workspace,
                    default_provider,
                    is_live,
                    is_input_ready,
                ) {
                    catalog.insert(binding.session_id.clone(), binding);
                }
            }
        }
    }

    for target in bindings.values() {
        if catalog.contains_key(target) {
            continue;
        }
        if let Some(config) = crate::manager::persisted_agent_config(target) {
            if let Some(binding) =
                agent_binding_from_config(&config, workspace, default_provider, false, false)
            {
                catalog.insert(binding.session_id.clone(), binding);
            }
        }
    }

    for assignment in assignments.values() {
        let WorkflowRoleAssignment::Agent { agent_id, .. } = assignment else {
            continue;
        };
        if catalog.contains_key(agent_id) {
            continue;
        }
        if let Some(config) = crate::manager::persisted_agent_config(agent_id) {
            if let Some(binding) =
                agent_binding_from_config(&config, workspace, default_provider, false, false)
            {
                catalog.insert(binding.session_id.clone(), binding);
            }
        }
    }

    catalog
}

fn agent_binding_from_config(
    config: &AgentConfig,
    workspace: &Path,
    default_provider: &str,
    is_live: bool,
    is_input_ready: bool,
) -> Option<AgentBinding> {
    let session_id = config.session_id.trim();
    if session_id.is_empty() {
        return None;
    }

    let provider = if config.provider.trim().is_empty() {
        default_provider.to_string()
    } else {
        config.provider.clone()
    };
    let cwd = if config.folder.trim().is_empty() {
        workspace.to_path_buf()
    } else {
        PathBuf::from(&config.folder)
    };

    Some(AgentBinding {
        session_id: session_id.to_string(),
        provider,
        cwd,
        resume_session: config.resume_session.clone(),
        is_live,
        is_input_ready,
    })
}

/// Drive a fresh run to completion or pause.
pub async fn drive_new_run(
    blueprint: Blueprint,
    run_id: String,
    run_root: PathBuf,
    workspace: PathBuf,
    default_provider: String,
    input: Value,
    bindings: HashMap<String, String>,
) -> Result<(), String> {
    drive_new_run_with_catalog(
        None,
        blueprint,
        run_id,
        run_root,
        workspace,
        default_provider,
        input,
        bindings,
        HashMap::new(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn drive_new_run_with_catalog(
    app: Option<tauri::AppHandle>,
    blueprint: Blueprint,
    run_id: String,
    run_root: PathBuf,
    workspace: PathBuf,
    default_provider: String,
    input: Value,
    bindings: HashMap<String, String>,
    agent_catalog: HashMap<String, AgentBinding>,
) -> Result<(), String> {
    let assignments = wardian_core::workflow::assignment::normalize_assignments(
        None,
        &bindings,
        InvocationKind::Manual,
    );
    drive_new_run_with_catalog_and_assignments(
        app,
        blueprint,
        run_id,
        run_root,
        workspace,
        default_provider,
        input,
        bindings,
        assignments,
        agent_catalog,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn drive_new_run_with_catalog_and_assignments(
    app: Option<tauri::AppHandle>,
    blueprint: Blueprint,
    run_id: String,
    run_root: PathBuf,
    workspace: PathBuf,
    default_provider: String,
    input: Value,
    bindings: HashMap<String, String>,
    assignments: WorkflowAssignments,
    agent_catalog: HashMap<String, AgentBinding>,
) -> Result<(), String> {
    let owner_id = format!("{}/{}", blueprint.id, run_id);
    write_run_invocation(
        &run_root,
        &default_provider,
        &workspace,
        &bindings,
        &assignments,
    )?;
    let exec = if let Some(app) = app {
        live_executor_with_catalog_assignments_and_app(
            app,
            workspace,
            default_provider,
            bindings,
            assignments,
            agent_catalog,
        )
    } else {
        live_executor_with_catalog_and_assignments(
            workspace,
            default_provider,
            bindings,
            assignments,
            agent_catalog,
        )
    }
    .with_owner_id(owner_id);
    Engine::start_with_id(&blueprint, run_id, input, &run_root, &exec)
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Resume an interrupted or paused run.
pub async fn drive_resume(
    blueprint: Blueprint,
    run_root: PathBuf,
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
) -> Result<(), String> {
    drive_resume_with_catalog(
        None,
        blueprint,
        run_root,
        workspace,
        default_provider,
        bindings,
        HashMap::new(),
    )
    .await
}

pub async fn drive_resume_with_catalog(
    app: Option<tauri::AppHandle>,
    blueprint: Blueprint,
    run_root: PathBuf,
    workspace: PathBuf,
    default_provider: String,
    bindings: HashMap<String, String>,
    agent_catalog: HashMap<String, AgentBinding>,
) -> Result<(), String> {
    let assignments = wardian_core::workflow::assignment::normalize_assignments(
        None,
        &bindings,
        InvocationKind::Manual,
    );
    let owner_id = run_root
        .file_name()
        .and_then(|value| value.to_str())
        .map(|run_id| format!("{}/{}", blueprint.id, run_id))
        .unwrap_or_else(|| format!("{}/resume", blueprint.id));
    let exec = if let Some(app) = app {
        live_executor_with_catalog_assignments_and_app(
            app,
            workspace,
            default_provider,
            bindings,
            assignments,
            agent_catalog,
        )
    } else {
        live_executor_with_catalog_and_assignments(
            workspace,
            default_provider,
            bindings,
            assignments,
            agent_catalog,
        )
    }
    .with_owner_id(owner_id);
    Engine::resume(&blueprint, &run_root, &exec)
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use wardian_core::engine::{event::EventKind, store::read_events, RunState, RunStatus};
    use wardian_core::models::{AgentConversationMode, BusyPolicy, WorkflowRoleAssignment};

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

    #[test]
    fn scan_interrupted_marks_running_runs() {
        let dir = tempfile::tempdir().unwrap();
        let run_root = dir.path().join("wf").join("run-1");
        std::fs::create_dir_all(&run_root).unwrap();
        let mut state = RunState::new("run-1", "wf");
        state.status = RunStatus::Running;
        wardian_core::engine::store::write_checkpoint(&run_root, &state).unwrap();

        let interrupted = scan_interrupted_runs(dir.path());
        assert_eq!(interrupted, vec![("wf".to_string(), "run-1".to_string())]);
    }

    #[test]
    fn run_invocation_round_trips_assignments() {
        let dir = tempfile::tempdir().unwrap();
        let run_root = dir.path().join("wf").join("run-1");
        let mut bindings = HashMap::new();
        bindings.insert("planner".to_string(), "agent-1".to_string());
        let mut assignments = WorkflowAssignments::new();
        assignments.insert(
            "planner".to_string(),
            WorkflowRoleAssignment::Agent {
                agent_id: "agent-1".to_string(),
                conversation: AgentConversationMode::Current,
                busy_policy: BusyPolicy::Skip,
            },
        );

        write_run_invocation(
            &run_root,
            "mock",
            std::path::Path::new("/workspace"),
            &bindings,
            &assignments,
        )
        .unwrap();

        let invocation = read_run_invocation(&run_root).unwrap().unwrap();
        assert_eq!(invocation.provider, "mock");
        assert_eq!(invocation.bindings, bindings);
        assert_eq!(invocation.assignments, assignments);
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

        let run_id = wardian_core::engine::driver::new_run_id();
        let run_root = wardian_core::paths::workflow_run_dir(&blueprint.id, &run_id).unwrap();
        drive_new_run(
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
}
