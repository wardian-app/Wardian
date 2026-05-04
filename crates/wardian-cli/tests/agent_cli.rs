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
    upsert_agent_with_conn(
        &conn,
        &AgentUpsert {
            session_id: "uuid-3",
            session_name: "fork-coder",
            agent_class: "Coder",
            provider: "codex",
            workspace: Some("D:/Forks/Wardian"),
            project: Some("Wardian"),
            is_off: false,
            created_at: Some("2026-05-03T20:04:00.000Z"),
        },
    )
    .unwrap();
    update_agent_status_with_conn(&conn, "uuid-1", "Processing...", Some(111)).unwrap();
    update_agent_status_with_conn(&conn, "uuid-2", "Idle", None).unwrap();
    update_agent_status_with_conn(&conn, "uuid-3", "Idle", None).unwrap();
    dir
}

fn seed_legacy_home() -> TempDir {
    let dir = TempDir::new().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE agents (
            session_id TEXT PRIMARY KEY,
            session_name TEXT UNIQUE,
            agent_class TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            is_off BOOLEAN DEFAULT 0,
            last_status TEXT,
            last_pid INTEGER
        );
        CREATE TABLE events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT,
            event_type TEXT,
            payload TEXT,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agents (
            session_id,
            session_name,
            agent_class,
            created_at,
            is_off,
            last_status,
            last_pid
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            "legacy-uuid-1",
            "legacy-coder",
            "Coder",
            "2026-05-03T20:00:00.000Z",
            false,
            "Idle",
            42
        ],
    )
    .unwrap();
    dir
}

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
    assert!(stdout.contains("{\n"));
    assert!(stdout.contains(r#"  "schema": 1"#));
    assert!(stdout.contains(r#""name": "coder-a1""#));
    assert!(stdout.contains(r#""workspace": "D:/Development/Wardian""#));
    assert!(!stdout.contains(r#""status_source""#));
    assert!(!stdout.contains(r#""project""#));
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
    assert!(stdout.contains(r#""agents": ["#));
    assert!(stdout.contains("coder-a1"));
    assert!(stdout.contains("architect-a1"));
    assert!(stdout.contains("fork-coder"));
    assert!(!stdout.contains(r#""status_source""#));
}

#[test]
fn status_source_is_available_when_requested() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent", "coder-a1", "--fields", "name,status_source"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""name": "coder-a1""#));
    assert!(stdout.contains(r#""status_source": "persisted""#));
}

#[test]
fn legacy_state_db_is_migrated_before_cli_queries() {
    let home = seed_legacy_home();
    let output = Command::new(bin())
        .args(["agent", "legacy-coder", "--field", "status"])
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "idle\n");
}

#[test]
fn list_scope_workspace_uses_callers_workspace() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent", "list"])
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
    assert!(stdout.contains("coder-a1"));
    assert!(stdout.contains("architect-a1"));
    assert!(!stdout.contains("fork-coder"));
}

#[test]
fn list_workspace_filter_matches_exact_workspace() {
    let home = seed_home();
    let output = Command::new(bin())
        .args([
            "agent",
            "list",
            "--workspace",
            "D:/Forks/Wardian",
            "--fields",
            "name,workspace",
        ])
        .env("WARDIAN_HOME", home.path())
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("fork-coder"));
    assert!(stdout.contains(r#""workspace": "D:/Forks/Wardian""#));
    assert!(!stdout.contains("coder-a1"));
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
    assert!(stdout.contains(r#""name": "coder-a1""#));
    assert!(stdout.contains(r#""status": "processing""#));
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

#[test]
fn malformed_args_emit_json_error_and_exit_one() {
    let output = Command::new(bin())
        .args(["agent", "list", "--definitely-not-a-real-flag"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""schema":1"#));
    assert!(stderr.contains(r#""code":"generic""#));
}

#[test]
fn list_accepts_output_fields_after_subcommand() {
    let home = seed_home();
    let output = Command::new(bin())
        .args(["agent", "list", "--scope", "all", "--fields", "name,status"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#""name": "coder-a1""#));
    assert!(stdout.contains(r#""status": "processing""#));
    assert!(!stdout.contains(r#""uuid""#));
}
