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

A tiny automation for CLI round-trip tests.
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

fn seed_demo_automation(home: &TempDir) -> std::path::PathBuf {
    let automations_dir = home.path().join("library").join("automations");
    std::fs::create_dir_all(&automations_dir).unwrap();
    let path = automations_dir.join("demo.md");
    std::fs::write(&path, DEMO_BLUEPRINT).unwrap();
    path
}

fn automation_command(home: &TempDir, args: &[&str]) -> serde_json::Value {
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

fn automation_output(home: &TempDir, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).unwrap()
}

fn automation_failure(home: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap()
}

#[test]
fn automation_message_send_validates_launch_fields_and_exposes_recipient_contract() {
    let home = TempDir::new().unwrap();
    let path = home.path().join("delivery.md");
    let blueprint = DEMO_BLUEPRINT
        .replace("type: task", "type: message_send")
        .replace(
            "agent: role:planner",
            "recipient: '{{trigger.output.requesting_agent}}'",
        )
        .replace(
            "prompt: Plan the demo",
            "artifact_path: '.wardian-review/{{run.id}}/review.md'",
        );
    std::fs::write(&path, &blueprint).unwrap();
    let report = automation_command(&home, &["automation", "validate", path.to_str().unwrap()]);
    assert_eq!(report["ok"], true);
    let contract = automation_output(
        &home,
        &["automation", "node-types", "message_send", "--json"],
    );
    assert!(contract.contains("recipient"));
    assert!(contract.contains("artifact_path"));
    for invalid in [
        blueprint.replace(
            "trigger.output.requesting_agent",
            "nodes.plan.output.recipient",
        ),
        blueprint.replace("recipient:", "recipient_id:"),
        blueprint.replace("run.id", "run.untrusted"),
    ] {
        std::fs::write(&path, invalid).unwrap();
        let failure =
            automation_failure(&home, &["automation", "validate", path.to_str().unwrap()]);
        assert!(!failure.status.success());
    }
}

#[test]
fn automation_list_uses_declared_ids_and_reports_parse_errors_per_row() {
    let home = TempDir::new().unwrap();
    let automations_dir = home.path().join("library").join("automations");
    let nested_dir = automations_dir.join("nested");
    std::fs::create_dir_all(&nested_dir).unwrap();

    let declared_path = nested_dir.join("filename-does-not-match-id.md");
    let declared_blueprint = DEMO_BLUEPRINT
        .replace("id: demo", "id: declared-id")
        .replace("name: Demo", "name: Declared Name");
    std::fs::write(&declared_path, declared_blueprint).unwrap();

    let malformed_path = automations_dir.join("broken.md");
    std::fs::write(&malformed_path, "not an automation blueprint").unwrap();

    let listed = automation_command(&home, &["automation", "list"]);
    assert_eq!(listed["schema"], 1);
    let automations = listed["automations"].as_array().unwrap();
    assert_eq!(automations.len(), 2);

    let declared = automations
        .iter()
        .find(|automation| {
            automation["entry_ref"] == "automations/nested/filename-does-not-match-id.md"
        })
        .unwrap();
    assert_eq!(declared["blueprint_id"], "declared-id");
    assert_eq!(declared["name"], "Declared Name");
    assert_eq!(
        declared["automation_path"]
            .as_str()
            .unwrap()
            .replace('\\', "/"),
        declared_path.to_string_lossy().replace('\\', "/")
    );
    assert!(declared["automation_path"]
        .as_str()
        .unwrap()
        .parse::<std::path::PathBuf>()
        .unwrap()
        .is_absolute());
    assert!(declared["error"].is_null());

    let malformed = automations
        .iter()
        .find(|automation| automation["entry_ref"] == "automations/broken.md")
        .unwrap();
    assert!(malformed["blueprint_id"].is_null());
    assert_eq!(malformed["name"], "broken");
    assert_eq!(
        malformed["automation_path"]
            .as_str()
            .unwrap()
            .replace('\\', "/"),
        malformed_path.to_string_lossy().replace('\\', "/")
    );
    assert!(malformed["error"]
        .as_str()
        .unwrap()
        .contains("front-matter"));

    let pretty = automation_output(&home, &["automation", "list", "--pretty"]);
    assert!(pretty.contains("declared-id"));
    assert!(pretty.contains("<unparseable>"));
    assert!(serde_json::from_str::<serde_json::Value>(&pretty).is_err());
}

#[test]
fn automation_exec_runs_show_replay_round_trip() {
    let home = TempDir::new().unwrap();
    let automation_path = seed_demo_automation(&home);

    let exec = automation_command(
        &home,
        &[
            "automation",
            "exec",
            automation_path.to_str().unwrap(),
            "--executor",
            "mock",
        ],
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
        .join("automations")
        .join("demo")
        .join(run_id);
    assert!(run_dir.is_dir());
    assert!(run_dir.join("events.jsonl").is_file());
    assert!(run_dir.join("state.json").is_file());

    let runs = automation_command(&home, &["automation", "runs"]);
    let runs = runs["runs"].as_array().unwrap();
    assert!(runs.iter().any(|run| {
        run["blueprint_id"] == "demo" && run["run_id"] == run_id && run["status"] == exec["status"]
    }));

    let shown = automation_command(&home, &["automation", "run-show", "demo", run_id]);
    let shown_status = shown["state"]["status"].as_str().unwrap();
    assert!(matches!(
        shown_status,
        "completed" | "failed" | "awaiting_approval"
    ));
    assert!(!shown["events"].as_array().unwrap().is_empty());

    let replayed = automation_command(&home, &["automation", "replay", "demo", run_id]);
    assert_eq!(replayed["state"]["status"], shown["state"]["status"]);
}

#[test]
fn automation_schedule_add_list_pause_resume_run_now_remove_round_trip() {
    let home = TempDir::new().unwrap();
    seed_demo_automation(&home);

    let add = automation_command(
        &home,
        &[
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "demo",
            "--name",
            "HB",
            "--every",
            "60",
            "--workspace",
            home.path().to_str().unwrap(),
            "--input",
            "{\"symbol\":\"SPY\"}",
            "--bind",
            "analyst=mock",
        ],
    );
    assert_eq!(add["ok"], true);
    assert_eq!(add["schedule"]["blueprint_id"], "demo");
    assert_eq!(add["schedule"]["input"]["symbol"], "SPY");
    assert_eq!(add["schedule"]["bindings"]["analyst"], "mock");
    let id = add["schedule"]["id"].as_str().unwrap();

    let list = automation_command(&home, &["automation", "schedule", "list"]);
    assert_eq!(list["schedules"].as_array().unwrap().len(), 1);

    let pause = automation_command(&home, &["automation", "schedule", "pause", id]);
    assert_eq!(pause["ok"], true);
    let paused = automation_command(&home, &["automation", "schedule", "list"]);
    assert_eq!(paused["schedules"][0]["is_paused"], true);
    assert!(paused["schedules"][0]["next_run_epoch_ms"].is_null());

    let resume = automation_command(&home, &["automation", "schedule", "resume", id]);
    assert_eq!(resume["ok"], true);
    let resumed = automation_command(&home, &["automation", "schedule", "list"]);
    assert_eq!(resumed["schedules"][0]["is_paused"], false);
    assert!(resumed["schedules"][0]["next_run_epoch_ms"].is_number());

    let run_now = automation_command(&home, &["automation", "schedule", "run-now", id]);
    assert_eq!(run_now["ok"], true);

    let remove = automation_command(&home, &["automation", "schedule", "remove", id]);
    assert_eq!(remove["ok"], true);
    assert_eq!(remove["removed"], 1);
    let empty = automation_command(&home, &["automation", "schedule", "list"]);
    assert!(empty["schedules"].as_array().unwrap().is_empty());
}

#[test]
fn automation_schedule_weekly_defaults_repeat_every_and_persists_original_command() {
    let home = TempDir::new().unwrap();
    seed_demo_automation(&home);
    let assignments = serde_json::json!({
        "planner": {
            "target_type": "temporary_provider",
            "provider": "mock",
            "workspace": home.path().to_string_lossy(),
        }
    })
    .to_string();

    let add = automation_command(
        &home,
        &[
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "demo",
            "--name",
            "Weekly Software Updates to Discord",
            "--weekly",
            "Sun@12:00",
            "--workspace",
            home.path().to_str().unwrap(),
            "--assignments",
            assignments.as_str(),
        ],
    );

    assert_eq!(add["ok"], true);
    assert_eq!(add["schedule"]["schedule"]["schedule_type"], "weekly");
    assert_eq!(add["schedule"]["schedule"]["repeat_every"], 1);
    assert_eq!(add["schedule"]["schedule"]["days_of_week"][0], "Sun");

    let persisted = automation_command(&home, &["automation", "schedule", "list"]);
    assert_eq!(persisted["schedules"][0]["schedule"]["repeat_every"], 1);
}

#[test]
fn automation_schedule_weekly_accepts_explicit_repeat_every() {
    let home = TempDir::new().unwrap();
    seed_demo_automation(&home);

    let add = automation_command(
        &home,
        &[
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "demo",
            "--name",
            "Biweekly",
            "--weekly",
            "Sun@12:00",
            "--repeat-every",
            "2",
            "--workspace",
            home.path().to_str().unwrap(),
        ],
    );

    assert_eq!(add["schedule"]["schedule"]["repeat_every"], 2);
}

#[test]
fn automation_schedule_update_changes_weekly_repeat_every() {
    let home = TempDir::new().unwrap();
    seed_demo_automation(&home);

    let add = automation_command(
        &home,
        &[
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "demo",
            "--name",
            "Weekly",
            "--weekly",
            "Sun@12:00",
            "--repeat-every",
            "2",
            "--workspace",
            home.path().to_str().unwrap(),
        ],
    );
    let id = add["schedule"]["id"].as_str().unwrap();

    let updated = automation_command(
        &home,
        &[
            "automation",
            "schedule",
            "update",
            id,
            "--repeat-every",
            "3",
        ],
    );

    assert_eq!(updated["ok"], true);
    assert_eq!(updated["schedule"]["id"], id);
    assert_eq!(updated["schedule"]["schedule"]["schedule_type"], "weekly");
    assert_eq!(updated["schedule"]["schedule"]["repeat_every"], 3);

    let persisted = automation_command(&home, &["automation", "schedule", "list"]);
    assert_eq!(persisted["schedules"][0]["schedule"]["repeat_every"], 3);
}

#[test]
fn automation_schedule_rejects_zero_or_invalid_repeat_every() {
    let home = TempDir::new().unwrap();
    seed_demo_automation(&home);
    let base_args = [
        "automation",
        "schedule",
        "add",
        "--blueprint",
        "demo",
        "--name",
        "Weekly",
        "--weekly",
        "Sun@12:00",
        "--workspace",
    ];
    let workspace = home.path().to_str().unwrap();

    let mut zero_args = base_args.to_vec();
    zero_args.extend([workspace, "--repeat-every", "0"]);
    let zero = automation_failure(&home, &zero_args);
    assert!(!zero.status.success());
    assert!(
        String::from_utf8_lossy(&zero.stderr).contains("--repeat-every must be greater than zero")
    );

    let mut invalid_args = base_args.to_vec();
    invalid_args.extend([workspace, "--repeat-every", "not-a-number"]);
    let invalid = automation_failure(&home, &invalid_args);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid value"));

    let monthly = automation_failure(
        &home,
        &[
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "demo",
            "--name",
            "Monthly",
            "--monthly",
            "1@12:00",
            "--workspace",
            workspace,
            "--repeat-every",
            "2",
        ],
    );
    assert!(!monthly.status.success());

    let persisted = automation_command(&home, &["automation", "schedule", "list"]);
    assert!(persisted["schedules"].as_array().unwrap().is_empty());
}

#[test]
fn automation_schedule_bounds_repeat_every_before_persistence() {
    let home = TempDir::new().unwrap();
    seed_demo_automation(&home);
    let workspace = home.path().to_str().unwrap();

    let accepted = automation_command(
        &home,
        &[
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "demo",
            "--name",
            "Long Weekly",
            "--weekly",
            "Sun@12:00",
            "--repeat-every",
            "520",
            "--workspace",
            workspace,
        ],
    );
    assert_eq!(accepted["schedule"]["schedule"]["repeat_every"], 520);

    let rejected = automation_failure(
        &home,
        &[
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "demo",
            "--name",
            "Too Long Weekly",
            "--weekly",
            "Sun@12:00",
            "--repeat-every",
            "521",
            "--workspace",
            workspace,
        ],
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("no greater than 520"));

    let persisted = automation_command(&home, &["automation", "schedule", "list"]);
    assert_eq!(persisted["schedules"].as_array().unwrap().len(), 1);
    assert_eq!(persisted["schedules"][0]["schedule"]["repeat_every"], 520);
}

#[test]
fn automation_schedule_update_preserves_identity_and_unspecified_configuration() {
    let home = TempDir::new().unwrap();
    seed_demo_automation(&home);
    let second_workspace = home.path().join("second-workspace");
    std::fs::create_dir_all(&second_workspace).unwrap();
    let assignments = serde_json::json!({
        "planner": {
            "target_type": "temporary_provider",
            "provider": "gemini",
            "workspace": home.path().to_string_lossy(),
        }
    })
    .to_string();

    let add = automation_command(
        &home,
        &[
            "automation",
            "schedule",
            "add",
            "--blueprint",
            "demo",
            "--name",
            "Original",
            "--every",
            "60",
            "--workspace",
            home.path().to_str().unwrap(),
            "--input",
            r#"{"symbol":"SPY"}"#,
            "--assignments",
            assignments.as_str(),
        ],
    );
    let assignment_json = add["schedule"]["assignments"].to_string();
    assert!(assignment_json.contains("temporary_provider"));
    let id = add["schedule"]["id"].as_str().unwrap().to_string();

    let updated = automation_command(
        &home,
        &[
            "automation",
            "schedule",
            "update",
            &id,
            "--name",
            "Updated",
            "--daily",
            "09:30",
            "--workspace",
            second_workspace.to_str().unwrap(),
        ],
    );

    assert_eq!(updated["ok"], true);
    assert_eq!(updated["schedule"]["id"], id);
    assert_eq!(updated["schedule"]["name"], "Updated");
    assert_eq!(updated["schedule"]["schedule"]["schedule_type"], "daily");
    assert_eq!(updated["schedule"]["input"]["symbol"], "SPY");
    assert!(updated["schedule"]["assignments"]["planner"]["target_type"] == "temporary_provider");
    assert!(updated["schedule"]["workspace"]
        .as_str()
        .unwrap()
        .ends_with("second-workspace"));
}
