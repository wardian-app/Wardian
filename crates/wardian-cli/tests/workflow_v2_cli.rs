use std::process::Command;
use tempfile::TempDir;

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
      agent: role:planner
      prompt: Plan the demo
edges:
  - from: trigger-1
    to: plan
---

# Demo

A tiny v2 workflow for CLI round-trip tests.
"#;

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

fn seed_demo_workflow(home: &TempDir) -> std::path::PathBuf {
    let workflows_dir = home.path().join("library").join("workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    let path = workflows_dir.join("demo.md");
    std::fs::write(&path, DEMO_BLUEPRINT).unwrap();
    path
}

fn workflow_command(home: &TempDir, args: &[&str]) -> serde_json::Value {
    let output = Command::new(bin())
        .args(args)
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}\nstdout: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn workflow_v2_exec_runs_show_replay_round_trip() {
    let home = TempDir::new().unwrap();
    let workflow_path = seed_demo_workflow(&home);

    let exec = workflow_command(
        &home,
        &["workflow", "exec", workflow_path.to_str().unwrap()],
    );
    assert_eq!(exec["schema"], 1);
    assert_eq!(exec["ok"], true);
    assert_eq!(exec["blueprint_id"], "demo");
    assert_eq!(exec["executor"], "mock");
    let run_id = exec["run_id"].as_str().unwrap();
    assert!(!run_id.is_empty());

    let run_dir = home
        .path()
        .join("logs")
        .join("workflows")
        .join("demo")
        .join(run_id);
    assert!(run_dir.is_dir());
    assert!(run_dir.join("events.jsonl").is_file());
    assert!(run_dir.join("state.json").is_file());

    let runs = workflow_command(&home, &["workflow", "runs"]);
    let runs = runs["runs"].as_array().unwrap();
    assert!(runs.iter().any(|run| {
        run["blueprint_id"] == "demo" && run["run_id"] == run_id && run["status"] == exec["status"]
    }));

    let shown = workflow_command(&home, &["workflow", "run-show", "demo", run_id]);
    let shown_status = shown["state"]["status"].as_str().unwrap();
    assert!(matches!(
        shown_status,
        "completed" | "failed" | "awaiting_approval"
    ));
    assert!(!shown["events"].as_array().unwrap().is_empty());

    let replayed = workflow_command(&home, &["workflow", "replay", "demo", run_id]);
    assert_eq!(replayed["state"]["status"], shown["state"]["status"]);
}
