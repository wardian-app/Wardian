use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

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

fn run(home: &std::path::Path, args: &[&str], stdin: Option<&str>) -> std::process::Output {
    let mut command = Command::new(bin());
    command
        .args(args)
        .env("WARDIAN_HOME", home)
        .env_remove("WARDIAN_SESSION_ID");
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    command.output_with_stdin(stdin)
}

trait CommandWithStdin {
    fn output_with_stdin(&mut self, stdin: Option<&str>) -> std::process::Output;
}

impl CommandWithStdin for Command {
    fn output_with_stdin(&mut self, stdin: Option<&str>) -> std::process::Output {
        self.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = self.spawn().unwrap();
        if let Some(stdin) = stdin {
            let mut pipe = child.stdin.take().expect("stdin pipe");
            pipe.write_all(stdin.as_bytes()).unwrap();
        }
        child.wait_with_output().unwrap()
    }
}

fn assert_success_json(output: std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}\nstdout: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_success_text(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}\nstdout: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn assert_failure_json(output: std::process::Output) -> serde_json::Value {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stderr).unwrap()
}

fn seed_default_classes(home: &Path) {
    let defaults = wardian_core::classes::default_class_definitions();
    wardian_core::classes::save_class_definitions(home, &defaults).unwrap();
}

fn seed_agent(home: &Path, session_id: &str) {
    let conn = rusqlite::Connection::open(home.join("state.db")).unwrap();
    wardian_core::db::run_migrations(&conn).unwrap();
    wardian_core::db::upsert_agent_with_conn(
        &conn,
        &wardian_core::db::AgentUpsert {
            session_id,
            session_name: "reviewer-a1",
            agent_class: "Reviewer",
            provider: "codex",
            workspace: Some("<absolute-workspace-path>"),
            project: Some("Wardian"),
            is_off: false,
            created_at: Some("2026-07-08T00:00:00.000Z"),
        },
    )
    .unwrap();
}

#[test]
fn prompt_create_show_read_write_move_delete_round_trip() {
    let home = TempDir::new().unwrap();

    let created = assert_success_json(run(
        home.path(),
        &["library", "create", "prompts/triage.md", "--stdin"],
        Some("# Triage\n\nInitial prompt\n"),
    ));
    assert_eq!(created["ok"], true);
    assert_eq!(created["entry_ref"], "prompts/triage.md");

    let duplicate = assert_failure_json(run(
        home.path(),
        &["library", "create", "prompts/triage.md", "--stdin"],
        Some("duplicate"),
    ));
    assert_eq!(duplicate["error"]["code"], "already_exists");

    let shown = assert_success_json(run(
        home.path(),
        &["library", "show", "prompts/triage.md", "--content"],
        None,
    ));
    assert_eq!(shown["entry_ref"], "prompts/triage.md");
    assert_eq!(shown["kind"], "prompt");
    assert_eq!(shown["content"], "# Triage\n\nInitial prompt\n");

    let read = assert_success_text(run(
        home.path(),
        &["library", "read", "prompts/triage.md"],
        None,
    ));
    assert_eq!(read, "# Triage\n\nInitial prompt\n");

    let written = assert_success_json(run(
        home.path(),
        &["library", "write", "prompts/triage.md", "--stdin"],
        Some("# Triage\n\nUpdated prompt\n"),
    ));
    assert_eq!(written["ok"], true);

    let moved = assert_success_json(run(
        home.path(),
        &[
            "library",
            "move",
            "prompts/triage.md",
            "prompts/daily-triage.md",
        ],
        None,
    ));
    assert_eq!(moved["from_ref"], "prompts/triage.md");
    assert_eq!(moved["to_ref"], "prompts/daily-triage.md");

    let old_read = assert_failure_json(run(
        home.path(),
        &["library", "read", "prompts/triage.md"],
        None,
    ));
    assert_eq!(old_read["error"]["code"], "not_found");

    let deleted = assert_success_json(run(
        home.path(),
        &["library", "delete", "prompts/daily-triage.md"],
        None,
    ));
    assert_eq!(deleted["ok"], true);
    assert!(!home
        .path()
        .join("library")
        .join("prompts")
        .join("daily-triage.md")
        .exists());
}

#[test]
fn rejects_unindexable_entry_shapes_without_hiding_existing_entries() {
    let home = TempDir::new().unwrap();

    for entry_ref in ["prompts/audit", "workflows/audit.txt"] {
        let rejected = assert_failure_json(run(
            home.path(),
            &["library", "create", entry_ref, "--stdin"],
            Some("body"),
        ));
        assert_eq!(rejected["error"]["code"], "invalid_ref");
    }
    assert!(!home.path().join("library/prompts/audit").exists());
    assert!(!home.path().join("library/workflows/audit.txt").exists());

    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/parent", "--stdin"],
        Some("# Parent\n"),
    ));
    let nested = assert_failure_json(run(
        home.path(),
        &["library", "create", "skills/parent/child", "--stdin"],
        Some("# Child\n"),
    ));
    assert_eq!(nested["error"]["code"], "invalid_ref");

    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/group/child", "--stdin"],
        Some("# Group child\n"),
    ));
    let promoted = assert_failure_json(run(
        home.path(),
        &["library", "create", "skills/group", "--stdin"],
        Some("# Group parent\n"),
    ));
    assert_eq!(promoted["error"]["code"], "invalid_ref");

    let listed = assert_success_json(run(
        home.path(),
        &["library", "list", "skills", "--flat"],
        None,
    ));
    let refs: Vec<&str> = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["entry_ref"].as_str())
        .collect();
    assert_eq!(refs, vec!["skills/group/child", "skills/parent"]);
}

#[test]
fn list_flat_outputs_section_entries() {
    let home = TempDir::new().unwrap();
    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/review/planner", "--stdin"],
        Some("---\ndescription: Plans reviews\n---\n# Planner\n"),
    ));

    let listed = assert_success_json(run(
        home.path(),
        &["library", "list", "skills", "--flat"],
        None,
    ));

    assert_eq!(listed["schema"], 1);
    assert_eq!(listed["section"], "skills");
    assert!(listed.get("tree").is_none());
    assert_eq!(listed["entries"][0]["section"], "skills");
    assert_eq!(listed["entries"][0]["entry_ref"], "skills/review/planner");
    assert_eq!(listed["entries"][0]["description"], "Plans reviews");
}

#[test]
fn list_flat_without_section_combines_entries_without_index_payloads() {
    let home = TempDir::new().unwrap();
    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/planner", "--stdin"],
        Some("# Planner\n"),
    ));
    assert_success_json(run(
        home.path(),
        &["library", "create", "prompts/triage.md", "--stdin"],
        Some("# Triage\n"),
    ));

    let listed = assert_success_json(run(home.path(), &["library", "list", "--flat"], None));

    let refs: Vec<&str> = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["entry_ref"].as_str())
        .collect();
    assert!(refs.contains(&"skills/planner"));
    assert!(refs.contains(&"prompts/triage.md"));
    assert!(listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["section"].is_string()));
    for omitted in ["sections", "tree", "deployments", "orphans"] {
        assert!(listed.get(omitted).is_none(), "unexpected key: {omitted}");
    }
}

#[test]
fn workflow_show_includes_absolute_workflow_path_but_does_not_validate() {
    let home = TempDir::new().unwrap();
    assert_success_json(run(
        home.path(),
        &["library", "create", "workflows/review/audit.md", "--stdin"],
        Some("not valid workflow yaml"),
    ));

    let shown = assert_success_json(run(
        home.path(),
        &["library", "show", "workflows/review/audit.md"],
        None,
    ));

    let workflow_path = shown["workflow_path"].as_str().unwrap();
    assert!(
        workflow_path.ends_with("library/workflows/review/audit.md")
            || workflow_path.ends_with("library\\workflows\\review\\audit.md")
    );
    assert_eq!(shown["entry_ref"], "workflows/review/audit.md");
    assert!(shown.get("diagnostics").is_none());
}

#[test]
fn star_unstar_and_tags_update_metadata() {
    let home = TempDir::new().unwrap();
    assert_success_json(run(
        home.path(),
        &["library", "create", "prompts/triage.md", "--stdin"],
        Some("# Triage\n"),
    ));

    let starred = assert_success_json(run(
        home.path(),
        &["library", "star", "prompts/triage.md"],
        None,
    ));
    assert_eq!(starred["is_starred"], true);

    let tagged = assert_success_json(run(
        home.path(),
        &[
            "library",
            "tags",
            "prompts/triage.md",
            "--set",
            "ops",
            "--set",
            "review",
            "--set",
            "ops",
        ],
        None,
    ));
    assert_eq!(tagged["tags"], serde_json::json!(["ops", "review"]));

    let shown = assert_success_json(run(
        home.path(),
        &["library", "show", "prompts/triage.md"],
        None,
    ));
    assert_eq!(shown["is_starred"], true);
    assert_eq!(shown["tags"], serde_json::json!(["ops", "review"]));

    let unstarred = assert_success_json(run(
        home.path(),
        &["library", "unstar", "prompts/triage.md"],
        None,
    ));
    assert_eq!(unstarred["is_starred"], false);
}

#[test]
fn deploy_reconciles_complete_target_set() {
    let home = TempDir::new().unwrap();
    seed_default_classes(home.path());
    seed_agent(home.path(), "agent-1");
    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/review/planner", "--stdin"],
        Some("# Planner\n"),
    ));

    let deployed = assert_success_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "user:global,class:Reviewer,agent:agent-1",
        ],
        None,
    ));
    assert_eq!(deployed["ok"], true);
    assert_eq!(deployed["outcome"]["added"], 3);
    assert!(home
        .path()
        .join("common")
        .join(".agents")
        .join("skills")
        .join("planner")
        .join("SKILL.md")
        .exists());
    assert!(home
        .path()
        .join("classes")
        .join("Reviewer")
        .join(".agents")
        .join("skills")
        .join("planner")
        .join("SKILL.md")
        .exists());
    assert!(home
        .path()
        .join("agents")
        .join("agent-1")
        .join(".agents")
        .join("skills")
        .join("planner")
        .join("SKILL.md")
        .exists());

    let deployments = assert_success_json(run(
        home.path(),
        &["library", "deployments", "skills/review/planner"],
        None,
    ));
    assert_eq!(deployments["targets"].as_array().unwrap().len(), 3);

    let narrowed = assert_success_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "class:Reviewer",
        ],
        None,
    ));
    assert_eq!(narrowed["outcome"]["removed"], 2);
    assert!(!home
        .path()
        .join("common")
        .join(".agents")
        .join("skills")
        .join("planner")
        .exists());
    assert!(!home
        .path()
        .join("agents")
        .join("agent-1")
        .join(".agents")
        .join("skills")
        .join("planner")
        .exists());
    assert!(home
        .path()
        .join("classes")
        .join("Reviewer")
        .join(".agents")
        .join("skills")
        .join("planner")
        .exists());
}

#[test]
fn deploy_deduplicates_targets_and_clear_removes_the_final_deployment() {
    let home = TempDir::new().unwrap();
    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/review/planner", "--stdin"],
        Some("# Planner\n"),
    ));

    let deployed = assert_success_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "user:global,user:global",
        ],
        None,
    ));
    assert_eq!(deployed["targets"].as_array().unwrap().len(), 1);
    assert_eq!(deployed["outcome"]["added"], 1);

    let cleared = assert_success_json(run(
        home.path(),
        &["library", "deploy", "skills/review/planner", "--clear"],
        None,
    ));
    assert_eq!(cleared["targets"], serde_json::json!([]));
    assert_eq!(cleared["outcome"]["removed"], 1);
    assert!(!home.path().join("common/.agents/skills/planner").exists());
}

#[test]
fn deploy_rejects_unknown_targets_without_creating_directories() {
    let home = TempDir::new().unwrap();
    seed_default_classes(home.path());
    seed_agent(home.path(), "agent-1");
    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/review/planner", "--stdin"],
        Some("# Planner\n"),
    ));

    let unknown_class = assert_failure_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "class:Reveiwer",
        ],
        None,
    ));
    assert_eq!(unknown_class["error"]["code"], "invalid_target");
    assert!(!home.path().join("classes").join("Reveiwer").exists());

    let unknown_agent = assert_failure_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "agent:missing-agent",
        ],
        None,
    ));
    assert_eq!(unknown_agent["error"]["code"], "invalid_target");
    assert!(!home.path().join("agents").join("missing-agent").exists());

    let agent_name = assert_failure_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "agent:reviewer-a1",
        ],
        None,
    ));
    assert_eq!(agent_name["error"]["code"], "invalid_target");
    assert!(!home.path().join("agents").join("reviewer-a1").exists());
}

#[test]
fn deploy_rejects_empty_target_list_without_removing_existing_deployments() {
    let home = TempDir::new().unwrap();
    seed_default_classes(home.path());
    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/review/planner", "--stdin"],
        Some("# Planner\n"),
    ));
    assert_success_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "user:global,class:Reviewer",
        ],
        None,
    ));

    let empty_targets = assert_failure_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "",
        ],
        None,
    ));
    assert_eq!(empty_targets["error"]["code"], "invalid_target");

    let deployments = assert_success_json(run(
        home.path(),
        &["library", "deployments", "skills/review/planner"],
        None,
    ));
    assert_eq!(deployments["targets"].as_array().unwrap().len(), 2);
    assert!(home
        .path()
        .join("common")
        .join(".agents")
        .join("skills")
        .join("planner")
        .exists());
    assert!(home
        .path()
        .join("classes")
        .join("Reviewer")
        .join(".agents")
        .join("skills")
        .join("planner")
        .exists());
}

#[test]
fn orphans_list_and_delete_remove_unresolved_deployment() {
    let home = TempDir::new().unwrap();
    let orphan = home
        .path()
        .join("classes")
        .join("Reviewer")
        .join(".agents")
        .join("skills")
        .join("ghost");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("SKILL.md"), "stale").unwrap();

    let orphans = assert_success_json(run(home.path(), &["library", "orphans"], None));
    assert_eq!(orphans["orphans"][0]["target_type"], "class");
    assert_eq!(orphans["orphans"][0]["target_id"], "Reviewer");
    assert_eq!(orphans["orphans"][0]["skill_name"], "ghost");

    let deleted = assert_success_json(run(
        home.path(),
        &[
            "library",
            "orphan",
            "delete",
            "--target",
            "class:Reviewer",
            "--skill",
            "ghost",
        ],
        None,
    ));
    assert_eq!(deleted["ok"], true);
    assert!(!orphan.exists());
}

#[test]
fn orphan_delete_rejects_healthy_deployment_without_removing_it() {
    let home = TempDir::new().unwrap();
    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/review/planner", "--stdin"],
        Some("# Planner\n"),
    ));
    assert_success_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "user:global",
        ],
        None,
    ));

    let rejected = assert_failure_json(run(
        home.path(),
        &[
            "library",
            "orphan",
            "delete",
            "--target",
            "user:global",
            "--skill",
            "planner",
        ],
        None,
    ));
    assert_eq!(rejected["error"]["code"], "not_found");
    assert!(home
        .path()
        .join("common/.agents/skills/planner/SKILL.md")
        .is_file());
}

#[test]
fn class_create_delete_and_restore_default_are_class_aware() {
    let home = TempDir::new().unwrap();

    let created = assert_success_json(run(
        home.path(),
        &["library", "create", "classes/PairProgrammer", "--stdin"],
        Some("# Pair Programmer\n\nCollaborates on code.\n"),
    ));
    assert_eq!(created["entry_ref"], "classes/PairProgrammer");
    assert!(home
        .path()
        .join("classes")
        .join("PairProgrammer")
        .join("AGENTS.md")
        .is_file());

    let class_move = assert_failure_json(run(
        home.path(),
        &[
            "library",
            "move",
            "classes/PairProgrammer",
            "classes/NewName",
        ],
        None,
    ));
    assert_eq!(class_move["error"]["code"], "not_supported");

    let deleted = assert_success_json(run(
        home.path(),
        &["library", "delete", "classes/PairProgrammer"],
        None,
    ));
    assert_eq!(deleted["ok"], true);
    assert!(!home.path().join("classes").join("PairProgrammer").exists());

    let defaults = wardian_core::classes::default_class_definitions();
    wardian_core::classes::save_class_definitions(home.path(), &defaults).unwrap();
    std::fs::create_dir_all(home.path().join("classes").join("Reviewer")).unwrap();
    std::fs::write(
        home.path()
            .join("classes")
            .join("Reviewer")
            .join("AGENTS.md"),
        "# Edited",
    )
    .unwrap();

    let restored = assert_success_json(run(
        home.path(),
        &["library", "restore-default", "classes/Reviewer"],
        None,
    ));
    assert_eq!(restored["ok"], true);
    assert!(std::fs::read_to_string(
        home.path()
            .join("classes")
            .join("Reviewer")
            .join("AGENTS.md")
    )
    .unwrap()
    .contains("Skeptical Auditor"));
}

#[test]
fn fresh_home_default_classes_support_cli_access_and_deployment() {
    let home = TempDir::new().unwrap();
    assert_success_json(run(
        home.path(),
        &["library", "create", "skills/review/planner", "--stdin"],
        Some("# Planner\n"),
    ));

    let deployed = assert_success_json(run(
        home.path(),
        &[
            "library",
            "deploy",
            "skills/review/planner",
            "--targets",
            "class:Reviewer",
        ],
        None,
    ));
    assert_eq!(deployed["outcome"]["added"], 1);

    let listed = assert_success_json(run(
        home.path(),
        &["library", "list", "classes", "--flat"],
        None,
    ));
    assert!(listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["entry_ref"] == "classes/Reviewer"));
    assert_success_json(run(
        home.path(),
        &["library", "show", "classes/Reviewer"],
        None,
    ));
    assert!(assert_success_text(run(
        home.path(),
        &["library", "read", "classes/Reviewer"],
        None,
    ))
    .contains("Skeptical Auditor"));
    assert_success_json(run(
        home.path(),
        &["library", "write", "classes/Reviewer", "--stdin"],
        Some("# Edited Reviewer\n"),
    ));

    let root = home.path().join("classes/Reviewer");
    assert_eq!(
        std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        "# Edited Reviewer\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("GEMINI.md")).unwrap(),
        "@AGENTS.md\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
        "@AGENTS.md\n"
    );
}
