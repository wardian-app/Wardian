use std::path::Path;
use wardian_core::engine::{
    NotifyRequest, ScriptRequest, ShellRequest, StateRequest, StepError, StepOutput,
};

/// Run a shell command in the run workspace, or `req.cwd` when supplied.
/// Captures exit code, stdout, and stderr.
pub async fn run_shell(ws: &Path, req: &ShellRequest) -> Result<StepOutput, StepError> {
    let cwd = req.cwd.as_deref().map(Path::new).unwrap_or(ws);
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };

    let mut command = crate::utils::process::new_silent_command(shell);
    let output = command
        .arg(flag)
        .arg(&req.command)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|err| StepError::new(err.to_string()))?;

    Ok(StepOutput(serde_json::json!({
        "exit_code": output.status.code().unwrap_or(-1),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    })))
}

/// Run `runtime path` in the workspace and capture exit code, stdout, and
/// stderr.
pub async fn run_script(ws: &Path, req: &ScriptRequest) -> Result<StepOutput, StepError> {
    let mut command = crate::utils::process::new_silent_command(&req.runtime);
    let output = command
        .arg(&req.path)
        .current_dir(ws)
        .output()
        .await
        .map_err(|err| StepError::new(err.to_string()))?;

    Ok(StepOutput(serde_json::json!({
        "exit_code": output.status.code().unwrap_or(-1),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    })))
}

/// Surface a notify step. 5a logs it; desktop notification wiring is deferred.
pub fn notify(req: &NotifyRequest) -> Result<(), StepError> {
    crate::utils::logging::log_debug(&format!(
        "[workflow-v2] notify {}: {}",
        req.node, req.message
    ));
    Ok(())
}

/// State ops return the value that the engine stores as the node output.
pub fn state_op(req: &StateRequest) -> Result<StepOutput, StepError> {
    let value = match req.op.as_str() {
        "set" | "merge" => req.entries.clone(),
        "delete" => serde_json::json!({}),
        other => return Err(StepError::new(format!("unknown state op: {other}"))),
    };

    Ok(StepOutput(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::engine::{ShellRequest, StateRequest};

    #[tokio::test]
    async fn shell_runs_and_reports_exit_code() {
        let req = ShellRequest {
            node: "s".into(),
            command: if cfg!(windows) {
                "exit 0".into()
            } else {
                "true".into()
            },
            cwd: None,
        };
        let out = run_shell(std::path::Path::new("."), &req).await.unwrap();
        assert_eq!(out.0["exit_code"], 0);
    }

    #[test]
    fn state_set_returns_entries() {
        let req = StateRequest {
            node: "st".into(),
            op: "set".into(),
            entries: serde_json::json!({"k":"v"}),
        };
        let out = state_op(&req).unwrap();
        assert_eq!(out.0["k"], "v");
    }
}
