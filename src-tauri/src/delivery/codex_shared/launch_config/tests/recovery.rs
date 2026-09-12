use super::*;

#[test]
fn crash_before_config_publication_is_a_noop_restore() {
    for original in [None, Some("model='baseline'\n")] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        if let Some(text) = original {
            fs::write(&path, text).unwrap();
        }
        let before = storage::read_snapshot(&path).unwrap();
        let mut document = parse_config(&before).unwrap();
        let changes = leaves::apply(
            &mut document,
            leaves::parse(&args(&["model='selected'"])).unwrap(),
        )
        .unwrap();
        let intent = journal::Journal::new(home.path(), changes)
            .unwrap()
            .encode(home.path())
            .unwrap();
        storage::publish(
            &home.path().join(journal::FILE),
            &None,
            intent.as_bytes(),
            None,
        )
        .unwrap();
        recover_launch_config(home.path()).unwrap();
        assert_eq!(storage::read_snapshot(&path).unwrap(), before);
        assert!(!home.path().join(journal::FILE).exists());
    }
}

#[test]
fn next_start_recovers_crash_after_overlay_without_retaining_removed_options() {
    let home = tempfile::tempdir().unwrap();
    let mut interrupted = prepare_launch_config(
        home.path(),
        &args(&[
            "web_search='live'",
            "developer_instructions='old memory'",
            "sandbox_workspace_write.writable_roots=['/old-generated']",
        ]),
    )
    .unwrap();
    // Simulate process loss after config publication: retain on-disk intent,
    // without running this instance's Drop cleanup.
    interrupted.token = None;
    let mut next = prepare_launch_config(home.path(), &args(&["model='next'"])).unwrap();
    let config = read(home.path());
    assert!(config.get("web_search").is_none());
    assert!(config.get("developer_instructions").is_none());
    assert!(config["sandbox_workspace_write"]
        .get("writable_roots")
        .is_none());
    assert_eq!(config["model"].as_str(), Some("next"));
    next.restore().unwrap();
}

#[test]
fn recovery_precedes_recomputation_of_roots_from_the_user_baseline() {
    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join("config.toml"),
        "[sandbox_workspace_write]\nwritable_roots=['/user']\n",
    )
    .unwrap();
    let mut interrupted = prepare_launch_config(
        home.path(),
        &args(&["sandbox_workspace_write.writable_roots=['/user','/old']"]),
    )
    .unwrap();
    interrupted.token = None;
    recover_launch_config(home.path()).unwrap(); // Mandatory caller step before runtime args generation.
    let mut roots = read(home.path())["sandbox_workspace_write"]["writable_roots"]
        .as_array()
        .unwrap()
        .clone();
    roots.push("/new");
    let next_args = args(&[&format!("sandbox_workspace_write.writable_roots={roots}")]);
    let mut next = prepare_launch_config(home.path(), &next_args).unwrap();
    let config = read(home.path());
    let actual = config["sandbox_workspace_write"]["writable_roots"]
        .as_array()
        .unwrap();
    assert_eq!(
        actual
            .iter()
            .map(|root| root.as_str().unwrap())
            .collect::<Vec<_>>(),
        ["/user", "/new"]
    );
    next.restore().unwrap();
}

#[test]
fn crash_after_restored_config_before_journal_removal_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("config.toml"), "model='baseline'\n").unwrap();
    let mut guard = prepare_launch_config(home.path(), &args(&["model='selected'"])).unwrap();
    let recorded = storage::read_snapshot(&home.path().join(journal::FILE)).unwrap();
    let intent = journal::Journal::decode(&recorded.as_ref().unwrap().text, home.path()).unwrap();
    let before = storage::read_snapshot(&home.path().join("config.toml")).unwrap();
    let mut document = parse_config(&before).unwrap();
    leaves::restore(&mut document, &intent.changes).unwrap();
    write_launch_config(
        home.path(),
        &before,
        &render(&document, &before).unwrap(),
        &recorded,
    )
    .unwrap();
    let restored = storage::read_snapshot(&home.path().join("config.toml")).unwrap();
    guard.token = None;
    recover_launch_config(home.path()).unwrap();
    assert_eq!(
        storage::read_snapshot(&home.path().join("config.toml")).unwrap(),
        restored
    );
    assert!(!home.path().join(journal::FILE).exists());
}

#[test]
fn stale_guard_cannot_restore_or_remove_the_next_tokens_journal() {
    let home = tempfile::tempdir().unwrap();
    let mut old = prepare_launch_config(home.path(), &args(&["model='old-startup'"])).unwrap();
    let mut current = prepare_launch_config(home.path(), &args(&["model='new-startup'"])).unwrap();
    let record = storage::read_snapshot(&home.path().join(journal::FILE)).unwrap();
    old.restore().unwrap();
    drop(old);
    assert_eq!(read(home.path())["model"].as_str(), Some("new-startup"));
    assert_eq!(
        storage::read_snapshot(&home.path().join(journal::FILE)).unwrap(),
        record
    );
    current.restore().unwrap();
}

#[test]
fn external_values_shapes_and_siblings_are_preserved_as_the_next_baseline() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("config.toml"), "model='baseline'\n").unwrap();
    let mut guard = prepare_launch_config(
        home.path(),
        &args(&[
            "model='selected'",
            "web_search='live'",
            "developer_instructions='temporary'",
            "sandbox_workspace_write.writable_roots=['/owned']",
        ]),
    )
    .unwrap();
    let external = "model = 'external' # user edit\ndeveloper_instructions = { foreign = true }\nweb_search='live'\n[sandbox_workspace_write]\nwritable_roots=['/owned','/external'] # external array edit\nforeign_sibling=true\n[mcp_servers.other]\ncommand='keep'\n";
    fs::write(home.path().join("config.toml"), external).unwrap();
    guard.restore().unwrap();
    let restored = read(home.path());
    assert_eq!(restored["model"].as_str(), Some("external"));
    assert!(restored["developer_instructions"].is_inline_table());
    assert!(restored.get("web_search").is_none());
    assert_eq!(
        restored["sandbox_workspace_write"]["writable_roots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        restored["sandbox_workspace_write"]["foreign_sibling"].as_bool(),
        Some(true)
    );
    assert_eq!(
        restored["mcp_servers"]["other"]["command"].as_str(),
        Some("keep")
    );
    let mut next = prepare_launch_config(home.path(), &args(&["model='new'"])).unwrap();
    next.restore().unwrap();
    assert_eq!(read(home.path())["model"].as_str(), Some("external"));
    assert!(fs::read_to_string(home.path().join("config.toml"))
        .unwrap()
        .contains("# user edit"));
}

#[test]
fn external_deletion_or_parent_shape_change_is_not_undone() {
    for external in [
        "# deleted all\n",
        "sandbox_workspace_write='external shape'\n",
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut guard = prepare_launch_config(
            home.path(),
            &args(&["sandbox_workspace_write.writable_roots=['/owned']"]),
        )
        .unwrap();
        fs::write(home.path().join("config.toml"), external).unwrap();
        guard.restore().unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join("config.toml")).unwrap(),
            external
        );
    }
}

#[test]
fn malformed_foreign_or_modified_journal_never_authorizes_config_changes() {
    let home = tempfile::tempdir().unwrap();
    let mut guard = prepare_launch_config(home.path(), &args(&["model='selected'"])).unwrap();
    let before = storage::read_snapshot(&home.path().join("config.toml")).unwrap();
    let record_path = home.path().join(journal::FILE);
    let record = fs::read_to_string(&record_path).unwrap();
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join(journal::FILE), &record).unwrap();
    assert!(recover_launch_config(other.path()).is_err());
    for malformed in ["{", "{\"schema_version\":999}"] {
        fs::write(&record_path, malformed).unwrap();
        assert!(guard.restore().is_err());
        assert_eq!(
            storage::read_snapshot(&home.path().join("config.toml")).unwrap(),
            before
        );
        assert_eq!(fs::read_to_string(&record_path).unwrap(), malformed);
    }
    let mut forged: serde_json::Value = serde_json::from_str(&record).unwrap();
    forged["changes"][0]["path"] = serde_json::json!(["mcp_servers", "unowned", "command"]);
    fs::write(&record_path, serde_json::to_string(&forged).unwrap()).unwrap();
    assert!(guard.restore().is_err());
    assert_eq!(
        storage::read_snapshot(&home.path().join("config.toml")).unwrap(),
        before
    );
    fs::write(&record_path, record).unwrap();
    guard.restore().unwrap();
}

#[test]
fn drop_fallback_restores_only_its_own_overlay() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("config.toml"), "model='baseline'\n").unwrap();
    {
        let _guard = prepare_launch_config(home.path(), &args(&["model='selected'"])).unwrap();
    }
    assert_eq!(read(home.path())["model"].as_str(), Some("baseline"));
    assert!(!home.path().join(journal::FILE).exists());
}
