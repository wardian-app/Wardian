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
