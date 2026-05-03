use std::process::Command;
use tempfile::TempDir;
use wardian_core::db::{
    run_migrations, update_agent_status_with_conn, upsert_agent_with_conn, AgentUpsert,
};

fn seed_home() -> TempDir {
    let dir = TempDir::new().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
    run_migrations(&conn).unwrap();
    upsert_agent_with_conn(
        &conn,
        &AgentUpsert {
            session_id: "uuid-1",
            session_name: "coder-a1",
            agent_class: "Coder",
            provider: "codex",
            workspace: Some("D:/Development/Wardian"),
            project: Some("Wardian"),
            is_off: false,
            created_at: Some("2026-05-03T20:00:00.000Z"),
        },
    )
    .unwrap();
    upsert_agent_with_conn(
        &conn,
        &AgentUpsert {
            session_id: "uuid-2",
            session_name: "architect-a1",
            agent_class: "Architect",
            provider: "claude",
            workspace: Some("D:/Development/Wardian"),
            project: Some("Wardian"),
            is_off: false,
            created_at: Some("2026-05-03T20:02:00.000Z"),
        },
    )
    .unwrap();
    update_agent_status_with_conn(&conn, "uuid-1", "Processing...", Some(111)).unwrap();
    update_agent_status_with_conn(&conn, "uuid-2", "Idle", None).unwrap();
    dir
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_wardian")
}

#[test]
fn self_lookup_returns_agent_json() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent"])
        .env("WARDIAN_HOME", home.path())
        .env("WARDIAN_SESSION_ID", "uuid-1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""schema":1"#));
    assert!(stdout.contains(r#""name":"coder-a1""#));
}

#[test]
fn not_in_session_exits_three() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent"])
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_in_session""#));
}

#[test]
fn peer_lookup_by_name_returns_agent() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent", "coder-a1", "--field", "status"])
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "processing\n");
}

#[test]
fn unknown_peer_exits_two() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent", "ghost"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"not_found""#));
}

#[test]
fn list_scope_all_returns_agents() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent", "list", "--scope", "all"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""agents":["#));
    assert!(stdout.contains("coder-a1"));
    assert!(stdout.contains("architect-a1"));
}

#[test]
fn fields_projection_omits_unrequested_fields() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent", "coder-a1", "--fields", "name,status"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""name":"coder-a1""#));
    assert!(stdout.contains(r#""status":"processing""#));
    assert!(!stdout.contains(r#""uuid""#));
}

#[test]
fn missing_db_exits_four() {
    let home = TempDir::new().unwrap();
    let output = Command::new(bin())
        .args(["agent", "list", "--scope", "all"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""code":"db_unavailable""#));
}
