use std::path::Path;
use tokio::process::Command;
use wardian_core::engine::{
    NotifyRequest, ScriptRequest, ShellRequest, StateRequest, StepError, StepOutput,
};

/// Run a shell command in the run workspace, or `req.cwd` when supplied.
/// Captures exit code, stdout, and stderr.
pub async fn run_shell(ws: &Path, req: &ShellRequest) -> Result<StepOutput, StepError> {
    let cwd = req.cwd.as_deref().map(Path::new).unwrap_or(ws);
    let output = shell_command(&req.command)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|err| StepError::new(err.to_string()))?;

    process_output(&req.node, "shell", output)
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("powershell.exe");
    shell
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(command);
    shell
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

/// Run `runtime path` in the workspace and capture exit code, stdout, and
/// stderr.
pub async fn run_script(ws: &Path, req: &ScriptRequest) -> Result<StepOutput, StepError> {
    let output = Command::new(&req.runtime)
        .arg(&req.path)
        .current_dir(ws)
        .output()
        .await
        .map_err(|err| StepError::new(err.to_string()))?;

    process_output(&req.node, "script", output)
}

fn process_output(
    node: &str,
    kind: &str,
    output: std::process::Output,
) -> Result<StepOutput, StepError> {
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let value = serde_json::json!({
        "exit_code": output.status.code().unwrap_or(-1),
        "stdout": stdout,
        "stderr": stderr,
    });

    if output.status.success() {
        return Ok(StepOutput(value));
    }

    let detail = if !stderr.trim().is_empty() {
        stderr.trim().to_string()
    } else if !stdout.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        "process exited without output".to_string()
    };
    Err(StepError::new(format!(
        "{kind} node {node} exited with exit code {exit_code}: {detail}"
    )))
}

/// Surface a notify step. 5a logs it; desktop notification wiring is deferred.
pub fn notify(req: &NotifyRequest) -> Result<(), StepError> {
    crate::utils::logging::log_debug(&format!("[workflow] notify {}: {}", req.node, req.message));
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

    #[tokio::test]
    async fn shell_nonzero_exit_fails_the_step() {
        let req = ShellRequest {
            node: "s".into(),
            command: if cfg!(windows) {
                "Write-Error bad; exit 7".into()
            } else {
                "echo bad >&2; exit 7".into()
            },
            cwd: None,
        };

        let err = run_shell(std::path::Path::new("."), &req)
            .await
            .expect_err("non-zero shell exit should fail the workflow step");

        assert!(err.0.contains("exit code 7"));
        assert!(err.0.contains("bad"));
    }

    #[tokio::test]
    async fn shell_preserves_single_quoted_arguments() {
        let temp = tempfile::tempdir().expect("temp dir");
        let req = ShellRequest {
            node: "s".into(),
            command: if cfg!(windows) {
                let script = temp.path().join("args.ps1");
                std::fs::write(
                    &script,
                    r#"if ($args[0] -ne "one two") { Write-Error "bad args: $($args -join ',')"; exit 9 }
Write-Output $args[0]
"#,
                )
                .expect("write script");
                format!("& '{}' 'one two'", script.to_string_lossy())
            } else {
                "printf '%s' 'one two'".into()
            },
            cwd: None,
        };

        let out = run_shell(temp.path(), &req).await.unwrap();
        assert_eq!(out.0["stdout"].as_str().unwrap().trim(), "one two");
    }

    #[tokio::test]
    async fn script_nonzero_exit_fails_the_step() {
        let temp = tempfile::tempdir().expect("temp dir");
        let script_path = if cfg!(windows) {
            "/C echo bad 1>&2 & exit 6".to_string()
        } else {
            let path = temp.path().join("fail.sh");
            std::fs::write(&path, "echo bad >&2\nexit 6\n").expect("write script");
            path.to_string_lossy().to_string()
        };
        let req = ScriptRequest {
            node: "script".into(),
            runtime: if cfg!(windows) {
                "cmd".into()
            } else {
                "sh".into()
            },
            path: script_path,
        };

        let err = run_script(temp.path(), &req)
            .await
            .expect_err("non-zero script exit should fail the workflow step");

        assert!(err.0.contains("exit code 6"));
        assert!(err.0.contains("bad"));
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
