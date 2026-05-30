use crate::workflow_v2::{runner::HeadlessAgentRunner, LiveStepExecutor};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wardian_core::engine::store::read_checkpoint;
use wardian_core::engine::{Engine, RunStatus};
use wardian_core::workflow::Blueprint;

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
    LiveStepExecutor::new(
        Arc::new(HeadlessAgentRunner),
        workspace,
        default_provider,
        bindings,
    )
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
    let exec = live_executor(workspace, default_provider, bindings);
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
    let exec = live_executor(workspace, default_provider, bindings);
    Engine::resume(&blueprint, &run_root, &exec)
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::engine::{RunState, RunStatus};

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
}
