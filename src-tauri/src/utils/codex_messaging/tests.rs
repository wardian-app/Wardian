use super::*;

mod reconciliation;

fn prepare(home: &Path, agent: &str, cli: bool) -> std::path::PathBuf {
    let codex = home.join("agents").join(agent).join("habitat/.codex");
    std::fs::create_dir_all(&codex).unwrap();
    if cli {
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            bin.join(super::super::cli_install::bundled_cli_file_name()),
            b"inert CLI fixture",
        )
        .unwrap();
    }
    codex
}

fn read_config(codex: &Path) -> DocumentMut {
    std::fs::read_to_string(codex.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn registration_binds_two_agents_and_is_byte_idempotent_without_approval_grants() {
    let temp = tempfile::tempdir().unwrap();
    for agent in ["agent-one", "agent-two"] {
        let codex = prepare(temp.path(), agent, true);
        let initial = "# user comment\r\nmodel = 'user-model'\r\n[mcp_servers.other]\r\ncommand = 'custom' # keep\r\n";
        std::fs::write(codex.join("config.toml"), initial).unwrap();
        assert_eq!(
            ensure_managed_messaging(temp.path(), agent).unwrap(),
            Registration::Updated
        );
        let config = read_config(&codex);
        let entry = &config["mcp_servers"][SERVER];
        let command = entry["command"].as_str().unwrap();
        assert!(Path::new(command).is_absolute());
        assert_eq!(
            Path::new(command).file_name().unwrap(),
            super::super::cli_install::bundled_cli_file_name()
        );
        assert_eq!(entry["env"]["WARDIAN_SESSION_ID"].as_str(), Some(agent));
        assert_eq!(entry["env"]["WARDIAN_HOME"].as_str(), temp.path().to_str());
        assert_eq!(entry["args"].as_array().unwrap().len(), 2);
        assert_eq!(entry["tool_timeout_sec"].as_integer(), Some(70));
        assert!(entry.get("tools").is_none());
        assert!(config.get("approval_policy").is_none());
        let bytes = std::fs::read(codex.join("config.toml")).unwrap();
        assert!(String::from_utf8(bytes.clone())
            .unwrap()
            .starts_with(initial));
        let record = std::fs::read(codex.join(RECORD)).unwrap();
        assert_eq!(
            ensure_managed_messaging(temp.path(), agent).unwrap(),
            Registration::Unchanged
        );
        assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), bytes);
        assert_eq!(std::fs::read(codex.join(RECORD)).unwrap(), record);
    }
}

#[test]
fn user_server_collision_preserves_disabled_approval_and_exact_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let codex = prepare(temp.path(), "agent", true);
    for original in [
        "# mine\r\n[mcp_servers.wardian]\r\ncommand = 'user-server'\r\nenabled = false\r\n[mcp_servers.wardian.tools.send_message]\r\napproval_mode = 'prompt'\r\n",
        "mcp_servers = { wardian = { command = 'custom', enabled = false } }\n",
        "mcp_servers = { other = { command = 'custom' } }\n",
    ] {
        std::fs::write(codex.join("config.toml"), original).unwrap();
        assert!(matches!(ensure_managed_messaging(temp.path(), "agent").unwrap(), Registration::Unavailable(_)));
        assert_eq!(std::fs::read_to_string(codex.join("config.toml")).unwrap(), original);
        assert!(!codex.join(RECORD).exists());
    }
}

#[test]
fn edits_to_owned_fields_and_removal_are_not_reclaimed() {
    let temp = tempfile::tempdir().unwrap();
    let codex = prepare(temp.path(), "agent", true);
    ensure_managed_messaging(temp.path(), "agent").unwrap();
    let baseline = std::fs::read_to_string(codex.join("config.toml")).unwrap();
    for field in [
        "command",
        "args",
        "WARDIAN_HOME",
        "WARDIAN_SESSION_ID",
        "remove",
    ] {
        let mut doc = baseline.parse::<DocumentMut>().unwrap();
        match field {
            "remove" => {
                doc["mcp_servers"].as_table_mut().unwrap().remove(SERVER);
            }
            "command" => doc["mcp_servers"][SERVER][field] = value("user-command"),
            "args" => doc["mcp_servers"][SERVER][field] = value(Array::new()),
            _ => doc["mcp_servers"][SERVER]["env"][field] = value("user-identity"),
        }
        let edited = doc.to_string();
        std::fs::write(codex.join("config.toml"), &edited).unwrap();
        assert!(matches!(
            ensure_managed_messaging(temp.path(), "agent").unwrap(),
            Registration::Unavailable(_)
        ));
        assert_eq!(
            std::fs::read_to_string(codex.join("config.toml")).unwrap(),
            edited
        );
    }
}

#[test]
fn home_relocation_refreshes_only_owned_fields_and_preserves_user_policy() {
    let old = tempfile::tempdir().unwrap();
    let old_codex = prepare(old.path(), "agent", true);
    ensure_managed_messaging(old.path(), "agent").unwrap();
    let mut config = read_config(&old_codex);
    let entry = &mut config["mcp_servers"][SERVER];
    entry["enabled"] = value(false);
    entry["tool_timeout_sec"] = value(999);
    entry["env"]["USER_VARIABLE"] = value("keep");
    entry["tools"] = Item::Table(Table::new());
    entry["tools"]["send_message"] = Item::Table(Table::new());
    entry["tools"]["send_message"]["approval_mode"] = value("prompt");
    let new = tempfile::tempdir().unwrap();
    let new_codex = prepare(new.path(), "agent", true);
    std::fs::write(new_codex.join("config.toml"), config.to_string()).unwrap();
    std::fs::copy(old_codex.join(RECORD), new_codex.join(RECORD)).unwrap();
    assert_eq!(
        ensure_managed_messaging(new.path(), "agent").unwrap(),
        Registration::Updated
    );
    let config = read_config(&new_codex);
    let entry = &config["mcp_servers"][SERVER];
    assert_eq!(entry["enabled"].as_bool(), Some(false));
    assert_eq!(entry["tool_timeout_sec"].as_integer(), Some(999));
    assert_eq!(
        entry["tools"]["send_message"]["approval_mode"].as_str(),
        Some("prompt")
    );
    assert_eq!(entry["env"]["USER_VARIABLE"].as_str(), Some("keep"));
    assert!(Path::new(entry["command"].as_str().unwrap())
        .starts_with(new.path().canonicalize().unwrap()));
    assert_eq!(entry["env"]["WARDIAN_HOME"].as_str(), new.path().to_str());
}

#[test]
fn missing_cli_leaves_files_untouched_and_registration_can_follow_installation() {
    let temp = tempfile::tempdir().unwrap();
    let codex = prepare(temp.path(), "agent", false);
    let original = "# keep me\n";
    std::fs::write(codex.join("config.toml"), original).unwrap();
    assert_eq!(
        ensure_managed_messaging(temp.path(), "agent").unwrap(),
        Registration::Unavailable("bundled native CLI is missing")
    );
    assert_eq!(
        std::fs::read_to_string(codex.join("config.toml")).unwrap(),
        original
    );
    assert!(!codex.join(RECORD).exists());
    prepare(temp.path(), "agent", true);
    assert_eq!(
        ensure_managed_messaging(temp.path(), "agent").unwrap(),
        Registration::Updated
    );
}

#[test]
fn invalid_context_and_malformed_toml_fail_without_repair_or_bootstrap_writes() {
    let temp = tempfile::tempdir().unwrap();
    for agent in ["", "..", "../other", "a/b", "a\\b", " agent"] {
        assert!(ensure_managed_messaging(temp.path(), agent).is_err());
    }
    assert!(ensure_managed_messaging(Path::new("relative-home"), "agent").is_err());
    let bootstrap = temp
        .path()
        .join("provider-bootstrap/codex/workspace/.codex");
    std::fs::create_dir_all(&bootstrap).unwrap();
    let codex = prepare(temp.path(), "agent", true);
    std::fs::write(codex.join("config.toml"), "[broken").unwrap();
    assert!(ensure_managed_messaging(temp.path(), "agent").is_err());
    assert_eq!(
        std::fs::read_to_string(codex.join("config.toml")).unwrap(),
        "[broken"
    );
    assert_eq!(std::fs::read_dir(bootstrap).unwrap().count(), 0);
}

#[test]
fn atomic_replacement_does_not_write_through_a_global_config_hardlink() {
    let temp = tempfile::tempdir().unwrap();
    let codex = prepare(temp.path(), "agent", true);
    let global = temp.path().join("global-config.toml");
    let original = "# global remains unchanged\nmodel = 'user-model'\n";
    std::fs::write(&global, original).unwrap();
    std::fs::hard_link(&global, codex.join("config.toml")).unwrap();
    assert_eq!(
        ensure_managed_messaging(temp.path(), "agent").unwrap(),
        Registration::Updated
    );
    assert_eq!(std::fs::read_to_string(global).unwrap(), original);
    assert!(read_config(&codex)["mcp_servers"].get(SERVER).is_some());
}

#[cfg(unix)]
#[test]
fn linked_config_is_rejected_without_touching_its_target() {
    let temp = tempfile::tempdir().unwrap();
    let codex = prepare(temp.path(), "agent", true);
    let global = temp.path().join("global-config.toml");
    std::fs::write(&global, "# global\n").unwrap();
    std::os::unix::fs::symlink(&global, codex.join("config.toml")).unwrap();
    assert!(ensure_managed_messaging(temp.path(), "agent").is_err());
    assert_eq!(std::fs::read_to_string(global).unwrap(), "# global\n");
}
