use std::process::Command;
use tempfile::TempDir;

fn bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_wardian-cli") {
        return path.into();
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_wardian_cli") {
        return path.into();
    }
    let exe = if cfg!(windows) {
        "wardian-cli.exe"
    } else {
        "wardian-cli"
    };
    std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(|deps| deps.parent())
        .unwrap()
        .join(exe)
}

fn seed_workflows(dir: &TempDir) {
    let workflows_dir = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    std::fs::write(
        workflows_dir.join("wf-abc123.json"),
        serde_json::json!({
            "id": "wf-abc123",
            "name": "Daily Review",
            "settings": {"max_iterations": 10, "on_limit_reached": "stop"},
            "nodes": [
                {"id": "n1", "type": "agent", "config": {}},
                {"id": "n2", "type": "agent", "config": {}},
            ],
            "role_mappings": {}
        })
        .to_string(),
    )
    .unwrap();
}

fn seed_workflow_with_role_mapping(dir: &TempDir) {
    let workflows_dir = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    std::fs::write(
        workflows_dir.join("wf-mapped.json"),
        serde_json::json!({
            "id": "wf-mapped",
            "name": "Mapped Review",
            "settings": {"max_iterations": 5, "on_limit_reached": "stop"},
            "nodes": [
                {
                    "id": "n1",
                    "type": "agent",
                    "name": "Reviewer",
                    "config": {"agent_class": "Coder"}
                }
            ],
            "role_mappings": {"primary_coder": "uuid-1"}
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn workflow_list_returns_summary_json() {
    let home = TempDir::new().unwrap();
    seed_workflows(&home);
    let output = Command::new(bin())
        .args(["workflow", "list"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""schema": 1"#));
    assert!(stdout.contains("Daily Review"));
    assert!(stdout.contains("wf-abc123"));
    assert!(stdout.contains(r#""node_count": 2"#));
}

#[test]
fn workflow_list_pretty_outputs_table() {
    let home = TempDir::new().unwrap();
    seed_workflows(&home);
    let output = Command::new(bin())
        .args(["workflow", "list", "--pretty"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("wf-abc123"));
    assert!(stdout.contains("Daily Review"));
    // pretty mode has no JSON braces
    assert!(!stdout.contains(r#""schema""#));
}

#[test]
fn workflow_list_empty_when_no_workflows_dir() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["workflow", "list"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""workflows": []"#));
}

#[test]
fn workflow_list_warns_about_malformed_workflow_files() {
    let home = TempDir::new().unwrap();
    let workflows_dir = home.path().join("workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    std::fs::write(workflows_dir.join("wf-bad.json"), "{not-json").unwrap();

    let output = Command::new(bin())
        .args(["workflow", "list"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""workflows": []"#));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("warning: skipped malformed workflow file"));
    assert!(stderr.contains("wf-bad.json"));
}

#[test]
fn workflow_show_by_id() {
    let home = TempDir::new().unwrap();
    seed_workflows(&home);
    let output = Command::new(bin())
        .args(["workflow", "show", "wf-abc123"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("wf-abc123"));
    assert!(stdout.contains("Daily Review"));
}

#[test]
fn workflow_show_outputs_full_definition() {
    let home = TempDir::new().unwrap();
    seed_workflow_with_role_mapping(&home);
    let output = Command::new(bin())
        .args(["workflow", "show", "wf-mapped"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""nodes": ["#));
    assert!(stdout.contains(r#""id": "n1""#));
    assert!(stdout.contains(r#""role_mappings": {"#));
    assert!(stdout.contains(r#""primary_coder": "uuid-1""#));
}

#[test]
fn workflow_show_by_name() {
    let home = TempDir::new().unwrap();
    seed_workflows(&home);
    let output = Command::new(bin())
        .args(["workflow", "show", "Daily Review"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("wf-abc123"));
}

#[test]
fn workflow_show_unknown_exits_two() {
    let home = TempDir::new().unwrap();
    seed_workflows(&home);
    let output = Command::new(bin())
        .args(["workflow", "show", "ghost-workflow"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_found""#));
}

#[test]
fn workflow_run_without_app_exits_six() {
    let home = TempDir::new().unwrap();
    seed_workflows(&home);
    let output = Command::new(bin())
        .args(["workflow", "run", "wf-abc123"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"app_not_running""#));
}

#[test]
fn workflow_stop_without_app_exits_six() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["workflow", "stop", "run-instance-xyz"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"app_not_running""#));
}
