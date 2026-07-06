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

fn seed_watchlist_home() -> TempDir {
    let dir = TempDir::new().unwrap();
    let watchlists = dir.path().join("watchlists");
    std::fs::create_dir_all(&watchlists).unwrap();
    std::fs::write(
        watchlists.join("index.json"),
        r#"{
          "version": 2,
          "teams": [{"id":"team-1","name":"Review","agentIds":["agent-1","agent-2"]}],
          "watchlists": [{"id":"list-1","name":"Main","entries":[{"type":"team","teamId":"team-1"}]}]
        }"#,
    )
    .unwrap();
    dir
}

fn seed_agent(
    home: &std::path::Path,
    session_id: &str,
    session_name: &str,
    workspace: Option<&str>,
) {
    let db_path = home.join("state.db");
    if !db_path.exists() {
        wardian_core::db::init_db_at_path(&db_path).unwrap();
    }
    let conn = rusqlite::Connection::open(db_path).unwrap();
    wardian_core::db::run_migrations(&conn).unwrap();
    wardian_core::db::upsert_agent_with_conn(
        &conn,
        &wardian_core::db::AgentUpsert {
            session_id,
            session_name,
            agent_class: "Coder",
            provider: "codex",
            workspace,
            project: None,
            is_off: false,
            created_at: Some("2026-07-05T00:00:00.000Z"),
        },
    )
    .unwrap();
}

fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env("WARDIAN_HOME", home)
        .env_remove("WARDIAN_SESSION_ID")
        .output()
        .unwrap()
}

fn assert_success(output: std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_failure(output: std::process::Output) -> serde_json::Value {
    assert!(!output.status.success(), "command unexpectedly succeeded");
    serde_json::from_slice(&output.stderr).unwrap()
}

fn read_index(home: &std::path::Path) -> serde_json::Value {
    let data = std::fs::read_to_string(home.join("watchlists").join("index.json")).unwrap();
    serde_json::from_str(&data).unwrap()
}

fn topology_has_edge(topology: &wardian_core::topology::Topology, a: &str, b: &str) -> bool {
    let (a, b) = wardian_core::topology::canonical_pair(a, b).unwrap();
    topology.edges.iter().any(|edge| edge.a == a && edge.b == b)
}

#[test]
fn team_list_outputs_schema_and_agent_ids() {
    let home = seed_watchlist_home();
    let output = Command::new(bin())
        .args(["team", "list"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["teams"][0]["id"], "team-1");
    assert_eq!(json["teams"][0]["agent_ids"][1], "agent-2");
}

#[test]
fn team_mutations_persist_canonical_state_and_seed_topology() {
    let home = TempDir::new().unwrap();
    seed_agent(
        home.path(),
        "agent-a",
        "Alpha",
        Some("<absolute-workspace-path>"),
    );
    seed_agent(
        home.path(),
        "agent-b",
        "Beta",
        Some("<absolute-workspace-path>"),
    );
    seed_agent(
        home.path(),
        "agent-c",
        "Gamma",
        Some("<absolute-workspace-path>"),
    );

    let created = assert_success(run(
        home.path(),
        &[
            "team", "create", "Writers", "--agent", "Alpha", "--agent", "Beta",
        ],
    ));
    assert_eq!(created["team"]["name"], "Writers");
    assert_eq!(
        created["team"]["agent_ids"],
        serde_json::json!(["agent-a", "agent-b"])
    );

    let index = read_index(home.path());
    assert_eq!(index["version"], 2);
    assert!(index["teams"][0].get("agentIds").is_some());
    assert!(index["teams"][0].get("agent_ids").is_none());
    assert_eq!(
        index["teams"][0]["agentIds"],
        serde_json::json!(["agent-a", "agent-b"])
    );

    let topology = wardian_core::topology::load_topology(home.path());
    assert_eq!(topology.edges.len(), 1);
    assert!(topology
        .neighbors("agent-a")
        .contains(&"agent-b".to_string()));

    assert_success(run(home.path(), &["team", "add", "Writers", "Gamma"]));
    let topology = wardian_core::topology::load_topology(home.path());
    assert!(topology
        .neighbors("agent-a")
        .contains(&"agent-c".to_string()));
    assert!(topology
        .neighbors("agent-b")
        .contains(&"agent-c".to_string()));

    assert_success(run(
        home.path(),
        &[
            "team",
            "split",
            "Writers",
            "--name",
            "Academic Writing",
            "--agent",
            "Beta",
            "--agent",
            "Gamma",
        ],
    ));
    let index = read_index(home.path());
    let teams = index["teams"].as_array().unwrap();
    assert_eq!(teams[0]["name"], "Writers");
    assert_eq!(teams[0]["agentIds"], serde_json::json!(["agent-a"]));
    assert_eq!(teams[1]["name"], "Academic Writing");
    assert_eq!(
        teams[1]["agentIds"],
        serde_json::json!(["agent-b", "agent-c"])
    );

    assert_success(run(
        home.path(),
        &["team", "rename", "Academic Writing", "Research"],
    ));
    assert_success(run(home.path(), &["team", "remove", "Research", "Gamma"]));
    let index = read_index(home.path());
    let research = index["teams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|team| team["name"] == "Research")
        .unwrap();
    assert_eq!(research["agentIds"], serde_json::json!(["agent-b"]));

    assert_success(run(home.path(), &["watchlist", "create", "Main"]));
    assert_success(run(
        home.path(),
        &["watchlist", "add-team", "Main", "Research"],
    ));
    assert_success(run(home.path(), &["team", "delete", "Research"]));
    let index = read_index(home.path());
    assert!(index["teams"]
        .as_array()
        .unwrap()
        .iter()
        .all(|team| team["name"] != "Research"));
    assert_eq!(index["watchlists"][0]["entries"], serde_json::json!([]));
}

#[test]
fn team_mutation_preserves_v2_suppressed_team_seed_pairs() {
    let home = seed_watchlist_home();
    seed_agent(
        home.path(),
        "agent-3",
        "Gamma",
        Some("<absolute-workspace-path>"),
    );
    std::fs::write(
        home.path().join("topology.json"),
        r#"{
          "version": 2,
          "edges": [],
          "ignored_pairs": [],
          "suppressed_seed_pairs": []
        }"#,
    )
    .unwrap();

    assert_success(run(home.path(), &["team", "add", "Review", "Gamma"]));

    let topology = wardian_core::topology::load_topology(home.path());
    assert_eq!(
        topology.version,
        wardian_core::topology::TOPOLOGY_SCHEMA_VERSION
    );
    assert!(!topology_has_edge(&topology, "agent-1", "agent-2"));
    assert!(topology_has_edge(&topology, "agent-1", "agent-3"));
    assert!(topology_has_edge(&topology, "agent-2", "agent-3"));
    assert!(topology.is_seed_suppressed("agent-1", "agent-2"));
}

#[test]
fn watchlist_mutations_manage_team_and_agent_entries() {
    let home = TempDir::new().unwrap();
    seed_agent(
        home.path(),
        "agent-a",
        "Alpha",
        Some("<absolute-workspace-path>"),
    );
    seed_agent(
        home.path(),
        "agent-b",
        "Beta",
        Some("<absolute-workspace-path>"),
    );
    assert_success(run(
        home.path(),
        &[
            "team", "create", "Review", "--agent", "Alpha", "--agent", "Beta",
        ],
    ));

    assert_success(run(home.path(), &["watchlist", "create", "Main"]));
    assert_success(run(
        home.path(),
        &["watchlist", "add-team", "Main", "Review"],
    ));
    assert_success(run(
        home.path(),
        &["watchlist", "add-agent", "Main", "Alpha"],
    ));

    let index = read_index(home.path());
    assert_eq!(
        index["watchlists"][0]["entries"].as_array().unwrap().len(),
        2
    );
    assert_eq!(index["watchlists"][0]["entries"][0]["type"], "team");
    assert_eq!(
        index["watchlists"][0]["entries"][0]["teamId"],
        index["teams"][0]["id"]
    );
    assert_eq!(index["watchlists"][0]["entries"][1]["agentId"], "agent-a");

    assert_success(run(
        home.path(),
        &["watchlist", "remove-agent", "Main", "Alpha"],
    ));
    assert_success(run(
        home.path(),
        &["watchlist", "remove-team", "Main", "Review"],
    ));
    assert_success(run(home.path(), &["watchlist", "rename", "Main", "Focus"]));
    let shown = assert_success(run(home.path(), &["watchlist", "show", "Focus"]));
    assert_eq!(shown["watchlist"]["name"], "Focus");
    assert_eq!(shown["watchlist"]["entries"], serde_json::json!([]));

    assert_success(run(home.path(), &["watchlist", "delete", "Focus"]));
    let index = read_index(home.path());
    assert_eq!(index["watchlists"], serde_json::json!([]));
}

#[test]
fn mutations_validate_duplicates_missing_agents_and_empty_teams() {
    let home = TempDir::new().unwrap();
    seed_agent(
        home.path(),
        "agent-a",
        "Alpha",
        Some("<absolute-workspace-path>"),
    );
    seed_agent(
        home.path(),
        "agent-b",
        "Beta",
        Some("<absolute-workspace-path>"),
    );

    let error = assert_failure(run(
        home.path(),
        &["team", "create", "Review", "--agent", "Missing"],
    ));
    assert_eq!(error["error"]["code"], "not_found");

    assert_success(run(
        home.path(),
        &["team", "create", "Review", "--agent", "agent-a"],
    ));
    let duplicate = assert_failure(run(
        home.path(),
        &["team", "create", "Review", "--agent", "agent-b"],
    ));
    assert_eq!(duplicate["error"]["code"], "duplicate_name");

    let empty = assert_failure(run(home.path(), &["team", "remove", "Review", "agent-a"]));
    assert_eq!(empty["error"]["code"], "empty_team");
}

#[test]
fn watchlist_show_outputs_entries() {
    let home = seed_watchlist_home();
    let output = Command::new(bin())
        .args(["watchlist", "show", "Main"])
        .env("WARDIAN_HOME", home.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["watchlist"]["id"], "list-1");
    assert_eq!(json["watchlist"]["entries"][0]["type"], "team");
    assert_eq!(json["watchlist"]["entries"][0]["team_id"], "team-1");
}
