use std::process::Command;
use tempfile::TempDir;
use wardian_core::control::{
    InteractionBodyRef, InteractionKind, InteractionRecord, InteractionStatus,
    InteractionTriggerPolicy,
};
use wardian_core::db::{
    run_migrations, upsert_agent_with_conn, upsert_interaction_record_with_conn, AgentUpsert,
};
use wardian_core::topology::{save_topology, Topology};

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

fn seed_agent(conn: &rusqlite::Connection, uuid: &str, name: &str, workspace: &str) {
    upsert_agent_with_conn(
        conn,
        &AgentUpsert {
            session_id: uuid,
            session_name: name,
            agent_class: "Coder",
            provider: "codex",
            workspace: Some(workspace),
            project: Some("Wardian"),
            is_off: false,
            created_at: Some("2026-07-03T10:00:00.000Z"),
        },
    )
    .unwrap();
}

fn message_record(id: &str, sender: &str, target: &str, created_at: &str) -> InteractionRecord {
    InteractionRecord {
        id: id.to_string(),
        kind: InteractionKind::Message,
        sender_session_id: Some(sender.to_string()),
        target_session_ids: vec![target.to_string()],
        status: InteractionStatus::Completed,
        trigger_policy: InteractionTriggerPolicy::NotifyOnly,
        body_ref: InteractionBodyRef::Inline { body: "hi".into() },
        parent_interaction_id: None,
        created_at: created_at.to_string(),
        updated_at: created_at.to_string(),
        completed_at: None,
    }
}

/// Three agents; one manual edge uuid-1<->uuid-2; traffic uuid-1<->uuid-3 (unmapped).
fn seed_home() -> TempDir {
    let dir = TempDir::new().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
    run_migrations(&conn).unwrap();
    seed_agent(&conn, "uuid-1", "coder-a1", "D:/ws");
    seed_agent(&conn, "uuid-2", "architect-a1", "D:/ws");
    seed_agent(&conn, "uuid-3", "fork-coder", "D:/other");
    upsert_interaction_record_with_conn(
        &conn,
        &message_record("int_1", "uuid-1", "uuid-3", "2026-07-03T09:00:00Z"),
    )
    .unwrap();

    let mut topology = Topology::default();
    topology.add_edge("uuid-1", "uuid-2", "2026-07-03T08:00:00Z");
    save_topology(dir.path(), &topology).unwrap();
    dir
}

fn run_graph(home: &TempDir, session: Option<&str>, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(bin());
    cmd.arg("graph").args(args).env("WARDIAN_HOME", home.path());
    match session {
        Some(session_id) => cmd.env("WARDIAN_SESSION_ID", session_id),
        None => cmd.env_remove("WARDIAN_SESSION_ID"),
    };
    cmd.output().unwrap()
}

fn stdout_json(output: &std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn show_returns_agents_edges_unmapped_and_ignored() {
    let home = seed_home();
    let body = stdout_json(&run_graph(&home, None, &["show"]));

    assert_eq!(body["schema"], 1);
    assert_eq!(body["agents"].as_array().unwrap().len(), 3);
    let edges = body["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["a"], "uuid-1");
    assert_eq!(edges[0]["b"], "uuid-2");
    let unmapped = body["unmapped_pairs"].as_array().unwrap();
    assert_eq!(unmapped.len(), 1);
    assert_eq!(unmapped[0]["a"], "uuid-1");
    assert_eq!(unmapped[0]["b"], "uuid-3");
    assert_eq!(body["ignored_pairs"].as_array().unwrap().len(), 0);
}

#[test]
fn show_excludes_ignored_pairs_from_unmapped() {
    let home = seed_home();
    let mut topology = Topology::default();
    topology.add_edge("uuid-1", "uuid-2", "2026-07-03T08:00:00Z");
    topology.ignore_pair("uuid-1", "uuid-3");
    save_topology(home.path(), &topology).unwrap();

    let body = stdout_json(&run_graph(&home, None, &["show"]));

    assert_eq!(body["unmapped_pairs"].as_array().unwrap().len(), 0);
    assert_eq!(body["ignored_pairs"].as_array().unwrap().len(), 1);
}

#[test]
fn neighbors_defaults_to_self_in_session() {
    let home = seed_home();
    let body = stdout_json(&run_graph(&home, Some("uuid-1"), &["neighbors"]));

    assert_eq!(body["agent_uuid"], "uuid-1");
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["uuid"], "uuid-2");
    assert_eq!(members[0]["name"], "architect-a1");
    assert_eq!(members[0]["reasons"][0], "manual");
}

#[test]
fn neighbors_reports_workspace_fallback_for_edgeless_agent() {
    let home = seed_home();
    // uuid-3 has no manual edges; fallback engages but no other agent shares D:/other.
    let body = stdout_json(&run_graph(&home, None, &["neighbors", "fork-coder"]));
    assert_eq!(body["members"].as_array().unwrap().len(), 0);

    // architect-a1 has a manual edge to coder-a1 only.
    let body = stdout_json(&run_graph(&home, None, &["neighbors", "architect-a1"]));
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["uuid"], "uuid-1");
}

#[test]
fn neighbors_without_session_or_arg_exits_three() {
    let home = seed_home();
    let output = run_graph(&home, None, &["neighbors"]);
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn activity_flags_unmapped_pairs() {
    let home = seed_home();
    let body = stdout_json(&run_graph(&home, None, &["activity"]));

    let pairs = body["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0]["a"], "uuid-1");
    assert_eq!(pairs[0]["b"], "uuid-3");
    assert_eq!(pairs[0]["last_message_at"], "2026-07-03T09:00:00Z");
    assert_eq!(pairs[0]["active_ask"], false);
    assert_eq!(pairs[0]["unmapped"], true);
}

#[test]
fn show_pretty_is_human_readable() {
    let home = seed_home();
    let output = run_graph(&home, None, &["show", "--pretty"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("coder-a1 <-> architect-a1"));
    assert!(stdout.contains("unmapped"));
}

use wardian_core::topology::load_topology;

#[test]
fn link_in_session_defaults_to_self_and_persists() {
    let home = seed_home();
    let body = stdout_json(&run_graph(&home, Some("uuid-1"), &["link", "fork-coder"]));

    assert_eq!(body["action"], "link");
    assert_eq!(body["changed"], true);
    assert_eq!(body["a"], "uuid-1");
    assert_eq!(body["b"], "uuid-3");

    let topology = load_topology(home.path());
    assert!(topology.neighbors("uuid-1").contains(&"uuid-3".to_string()));
}

#[test]
fn link_is_idempotent_with_changed_false() {
    let home = seed_home();
    // uuid-1 <-> uuid-2 already exists from seeding.
    let body = stdout_json(&run_graph(&home, Some("uuid-1"), &["link", "architect-a1"]));
    assert_eq!(body["changed"], false);
}

#[test]
fn link_in_session_rejects_foreign_pair() {
    let home = seed_home();
    let output = run_graph(&home, Some("uuid-1"), &["link", "uuid-2", "uuid-3"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("self_serve_required"));
    // Nothing was written.
    assert_eq!(load_topology(home.path()).edges.len(), 1);
}

#[test]
fn link_outside_session_allows_any_pair_but_requires_two_args() {
    let home = seed_home();
    let output = run_graph(&home, None, &["link", "uuid-2"]);
    assert_eq!(output.status.code(), Some(1));

    let body = stdout_json(&run_graph(&home, None, &["link", "uuid-2", "uuid-3"]));
    assert_eq!(body["changed"], true);
    assert_eq!(load_topology(home.path()).edges.len(), 2);
}

#[test]
fn stale_session_fails_closed() {
    let home = seed_home();
    let output = run_graph(&home, Some("uuid-gone"), &["link", "uuid-2", "uuid-3"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(load_topology(home.path()).edges.len(), 1);
}

#[test]
fn unlink_removes_edge_and_reports_not_linked_as_unchanged() {
    let home = seed_home();
    let body = stdout_json(&run_graph(
        &home,
        Some("uuid-1"),
        &["unlink", "architect-a1"],
    ));
    assert_eq!(body["changed"], true);
    assert!(load_topology(home.path()).edges.is_empty());

    let body = stdout_json(&run_graph(
        &home,
        Some("uuid-1"),
        &["unlink", "architect-a1"],
    ));
    assert_eq!(body["changed"], false);
}

#[test]
fn ignore_and_unignore_roundtrip() {
    let home = seed_home();
    let body = stdout_json(&run_graph(&home, Some("uuid-1"), &["ignore", "fork-coder"]));
    assert_eq!(body["changed"], true);
    assert!(load_topology(home.path()).is_ignored("uuid-1", "uuid-3"));

    // Ignored pair no longer appears as unmapped.
    let show = stdout_json(&run_graph(&home, None, &["show"]));
    assert_eq!(show["unmapped_pairs"].as_array().unwrap().len(), 0);

    let body = stdout_json(&run_graph(
        &home,
        Some("uuid-1"),
        &["unignore", "fork-coder"],
    ));
    assert_eq!(body["changed"], true);
    assert!(!load_topology(home.path()).is_ignored("uuid-1", "uuid-3"));
}

#[test]
fn link_unknown_agent_exits_two() {
    let home = seed_home();
    let output = run_graph(&home, Some("uuid-1"), &["link", "ghost"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn link_self_exits_one() {
    let home = seed_home();
    let output = run_graph(&home, None, &["link", "uuid-2", "uuid-2"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn link_creates_topology_file_on_fresh_home() {
    let dir = TempDir::new().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
    run_migrations(&conn).unwrap();
    seed_agent(&conn, "uuid-1", "coder-a1", "D:/ws");
    seed_agent(&conn, "uuid-2", "architect-a1", "D:/ws");

    let body = stdout_json(&run_graph(&dir, None, &["link", "uuid-1", "uuid-2"]));
    assert_eq!(body["changed"], true);
    assert!(dir.path().join("topology.json").exists());
}
