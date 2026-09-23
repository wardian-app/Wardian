use super::*;
use std::fs;

mod recovery;
#[cfg(windows)]
mod sharing;
#[cfg(windows)]
mod windows_long_paths;

fn args(assignments: &[&str]) -> Vec<String> {
    let mut args = vec!["app-server".into()];
    for assignment in assignments {
        args.extend(["-c".into(), (*assignment).into()]);
    }
    args
}

fn read(home: &Path) -> DocumentMut {
    fs::read_to_string(home.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn launch_values_restore_and_preserve_mcp_credentials_policy_and_comments() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let mcp = "# MCP local policy\n[mcp_servers.wardian]\ntool_timeout_sec = 999 # timeout\n[mcp_servers.wardian.env]\nTEST_SECRET = 'not-a-real-credential'\n[mcp_servers.wardian.tools.reply]\napproval_mode = 'approve' # explicit grant\n";
    fs::write(&path, format!("# user config\nmodel = 'old' # model note\n[sandbox_workspace_write]\nnetwork_access = false # sibling\n{mcp}")).unwrap();
    let mut guard = prepare_launch_config(home.path(), &args(&[
        r#"model="quoted \"model\"""#,
        r#"model_reasoning_effort="low""#,
        r#"approval_policy="on-request""#,
        r#"sandbox_mode="workspace-write""#,
        r#"approvals_reviewer="guardian_subagent""#,
        r#"sandbox_workspace_write.writable_roots=["/space root", "back\\slash"]"#,
        r#"developer_instructions="Use \"quotes\", 'apostrophes', \\ and café.\nNext\tline\r\n""#,
        r#"projects."/path.with=punctuation".trust_level="trusted""#,
    ])).unwrap();
    let config = read(home.path());
    assert_eq!(config["model"].as_str(), Some("quoted \"model\""));
    assert_eq!(config["model_reasoning_effort"].as_str(), Some("low"));
    assert_eq!(config["approval_policy"].as_str(), Some("on-request"));
    assert_eq!(config["sandbox_mode"].as_str(), Some("workspace-write"));
    assert_eq!(
        config["approvals_reviewer"].as_str(),
        Some("guardian_subagent")
    );
    assert_eq!(
        config["developer_instructions"].as_str(),
        Some("Use \"quotes\", 'apostrophes', \\ and café.\nNext\tline\r\n")
    );
    let roots = config["sandbox_workspace_write"]["writable_roots"]
        .as_array()
        .unwrap();
    assert_eq!(
        roots
            .iter()
            .map(|entry| entry.as_str().unwrap())
            .collect::<Vec<_>>(),
        ["/space root", "back\\slash"]
    );
    assert_eq!(
        config["projects"]["/path.with=punctuation"]["trust_level"].as_str(),
        Some("trusted")
    );
    let record = fs::read_to_string(home.path().join(journal::FILE)).unwrap();
    for forbidden in [
        "mcp_servers",
        "TEST_SECRET",
        "not-a-real-credential",
        "approval_mode",
    ] {
        assert!(!record.contains(forbidden));
    }
    guard.restore().unwrap();
    let restored = fs::read_to_string(&path).unwrap();
    assert_eq!(read(home.path())["model"].as_str(), Some("old"));
    assert!(read(home.path()).get("developer_instructions").is_none());
    assert!(restored.contains(mcp));
    for comment in ["# user config", "# model note", "# sibling"] {
        assert!(restored.contains(comment));
    }
    assert!(!home.path().join(journal::FILE).exists());
}

#[test]
fn tui_update_check_overlay_is_journaled_and_restores_model_effort_and_config() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let original = "# saved preferences\ncheck_for_update_on_startup = true\nmodel = 'gpt-5.4'\nmodel_reasoning_effort = 'low'\n[mcp_servers.wardian.env]\nWARDIAN_TOKEN = 'keep-private'\n";
    fs::write(&path, original).unwrap();

    let mut guard = prepare_tui_launch_config(
        home.path(),
        &args(&["model=\"gpt-5.6-luna\"", "model_reasoning_effort=\"high\""]),
    )
    .unwrap();

    let config = read(home.path());
    assert_eq!(config["check_for_update_on_startup"].as_bool(), Some(false));
    assert_eq!(config["model"].as_str(), Some("gpt-5.6-luna"));
    assert_eq!(config["model_reasoning_effort"].as_str(), Some("high"));
    assert_eq!(
        config["mcp_servers"]["wardian"]["env"]["WARDIAN_TOKEN"].as_str(),
        Some("keep-private")
    );

    let record = fs::read_to_string(home.path().join(journal::FILE)).unwrap();
    let journal: serde_json::Value = serde_json::from_str(&record).unwrap();
    let changes = journal["changes"].as_array().unwrap();
    let changed_paths = changes
        .iter()
        .map(|change| {
            change["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|part| part.as_str().unwrap())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        changed_paths,
        vec![
            vec!["check_for_update_on_startup"],
            vec!["model"],
            vec!["model_reasoning_effort"],
        ]
    );
    let update_change = changes
        .iter()
        .find(|change| change["path"] == serde_json::json!(["check_for_update_on_startup"]))
        .unwrap();
    assert_eq!(update_change["before"]["type"].as_str(), Some("boolean"));
    assert_eq!(update_change["before"]["value"].as_bool(), Some(true));
    assert_eq!(update_change["applied"]["type"].as_str(), Some("boolean"));
    assert_eq!(update_change["applied"]["value"].as_bool(), Some(false));
    assert!(!record.contains("mcp_servers"));
    assert!(!record.contains("WARDIAN_TOKEN"));
    assert!(!record.contains("keep-private"));

    guard.restore().unwrap();
    let restored = read(home.path());
    assert_eq!(
        restored["check_for_update_on_startup"].as_bool(),
        Some(true)
    );
    assert_eq!(restored["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(restored["model_reasoning_effort"].as_str(), Some("low"));
    assert_eq!(
        restored["mcp_servers"]["wardian"]["env"]["WARDIAN_TOKEN"].as_str(),
        Some("keep-private")
    );
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("# saved preferences"));
    assert!(!home.path().join(journal::FILE).exists());
}

#[test]
fn tui_update_check_overlay_removes_key_when_initially_absent() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let original = "model = 'gpt-5.4'\nmodel_reasoning_effort = 'high'\n";
    fs::write(&path, original).unwrap();

    let mut guard = prepare_tui_launch_config(
        home.path(),
        &args(&["model=\"gpt-5.4\"", "model_reasoning_effort=\"high\""]),
    )
    .unwrap();

    let config = read(home.path());
    assert_eq!(config["check_for_update_on_startup"].as_bool(), Some(false));
    let record = fs::read_to_string(home.path().join(journal::FILE)).unwrap();
    let journal: serde_json::Value = serde_json::from_str(&record).unwrap();
    let change = &journal["changes"][0];
    assert_eq!(
        change["path"],
        serde_json::json!(["check_for_update_on_startup"])
    );
    assert!(change["before"].is_null());
    assert_eq!(change["applied"]["value"].as_bool(), Some(false));

    guard.restore().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert!(read(home.path())
        .get("check_for_update_on_startup")
        .is_none());
    assert!(!home.path().join(journal::FILE).exists());
}

#[test]
fn boolean_launch_leaf_is_scoped_to_the_startup_update_setting() {
    let update_path = vec!["check_for_update_on_startup".to_string()];
    assert!(leaves::validate(&update_path, &leaves::Leaf::Boolean(false)).is_ok());
    assert!(leaves::validate(&update_path, &leaves::Leaf::String("false".into())).is_err());
    assert!(leaves::validate(&update_path, &leaves::Leaf::Strings(vec!["false".into()])).is_err());
    assert!(leaves::validate(&["model".to_string()], &leaves::Leaf::Boolean(false)).is_err());
}

#[test]
fn duplicate_keys_last_wins_and_repeated_restore_is_a_noop() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    fs::write(&path, "# CRLF\r\nmodel = 'old' # note\r\n").unwrap();
    let mut guard =
        prepare_launch_config(home.path(), &args(&["model='first'", "model='last'"])).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# CRLF\r\nmodel = \"last\" # note\r\n"
    );
    guard.restore().unwrap();
    let before = storage::read_snapshot(&path).unwrap();
    guard.restore().unwrap();
    drop(guard);
    assert_eq!(storage::read_snapshot(&path).unwrap(), before);
    assert_eq!(read(home.path())["model"].as_str(), Some("old"));
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 1);
}

#[test]
fn inline_table_siblings_and_original_root_values_survive_restore() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    fs::write(&path, "sandbox_workspace_write = { network_access = false, writable_roots = ['/user'] } # inline\n").unwrap();
    let mut guard = prepare_launch_config(
        home.path(),
        &args(&["sandbox_workspace_write.writable_roots=['/user','/owned']"]),
    )
    .unwrap();
    assert_eq!(
        read(home.path())["sandbox_workspace_write"]["network_access"].as_bool(),
        Some(false)
    );
    guard.restore().unwrap();
    assert_eq!(
        read(home.path())["sandbox_workspace_write"]["writable_roots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(fs::read_to_string(&path).unwrap().contains("# inline"));
}

#[test]
fn malformed_args_toml_and_unowned_keys_fail_without_mutation() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let invalid = [
        vec![],
        vec!["app-server".into(), "-c".into()],
        vec!["node".into(), "app-server".into(), "-c".into()],
        vec!["app-server".into(), "--listen".into(), "unix://".into()],
        args(&["model='valid'", "model=bare"]),
        args(&["model='a'\nweb_search='live'"]),
        args(&["[model]\nname='a'"]),
        args(&["# empty"]),
        args(&["model=["]),
        args(&["mcp_servers.wardian.env.SECRET='unowned'"]),
        args(&["model=42"]),
    ];
    for original in [None, Some("model='keep'\n")] {
        if let Some(text) = original {
            fs::write(&path, text).unwrap();
        }
        let before = storage::read_snapshot(&path).unwrap();
        for generated in &invalid {
            assert!(prepare_launch_config(home.path(), generated).is_err());
            assert_eq!(storage::read_snapshot(&path).unwrap(), before);
            assert_eq!(
                fs::read_dir(home.path()).unwrap().count(),
                usize::from(original.is_some())
            );
        }
    }
    for original in [
        b"model=[".as_slice(),
        b"\xff",
        b"sandbox_workspace_write='bad'\n",
    ] {
        fs::write(&path, original).unwrap();
        assert!(prepare_launch_config(
            home.path(),
            &args(&["sandbox_workspace_write.writable_roots=[]"])
        )
        .is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!home.path().join(journal::FILE).exists());
    }
}

#[test]
fn hardlinked_and_nonregular_config_or_journal_are_refused() {
    for destination in ["config.toml", journal::FILE] {
        let home = tempfile::tempdir().unwrap();
        let external = home.path().join("external");
        let path = home.path().join(destination);
        fs::write(&external, "model='keep'\n").unwrap();
        fs::hard_link(&external, &path).unwrap();
        assert!(prepare_launch_config(home.path(), &args(&["model='new'"])).is_err());
        assert_eq!(fs::read_to_string(&external).unwrap(), "model='keep'\n");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(prepare_launch_config(home.path(), &args(&["model='new'"])).is_err());
        assert!(path.is_dir());
    }
}

#[cfg(any(unix, windows))]
#[cfg_attr(
    windows,
    ignore = "requires Windows symlink privilege or Developer Mode"
)]
#[test]
fn symlink_config_and_journal_are_refused() {
    for destination in ["config.toml", journal::FILE] {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("target");
        let path = home.path().join(destination);
        fs::write(&target, "model='keep'\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &path).unwrap();
        assert!(prepare_launch_config(home.path(), &args(&["model='new'"])).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "model='keep'\n");
    }
}

#[test]
fn atomic_publication_rejects_identical_byte_replacement_and_cleans_temporary() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    fs::write(&path, "model='old'\n").unwrap();
    let before = storage::read_snapshot(&path).unwrap();
    fs::rename(&path, home.path().join("retained.toml")).unwrap();
    fs::write(&path, "model='old'\n").unwrap();
    assert!(storage::publish(&path, &before, b"model='new'\n", None).is_err());
    assert_eq!(read(home.path())["model"].as_str(), Some("old"));
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 2);
}
