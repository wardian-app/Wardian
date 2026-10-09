use super::*;

fn prepared() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join("agents/agent/habitat/.codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::create_dir_all(temp.path().join("bin")).unwrap();
    std::fs::write(
        temp.path().join("bin").join(messaging_server_file_name()),
        b"inert bundled executable",
    )
    .unwrap();
    std::fs::write(
        temp.path()
            .join("bin")
            .join(crate::utils::cli_install::bundled_cli_file_name()),
        b"inert bundled CLI",
    )
    .unwrap();
    ensure_managed_messaging(temp.path(), "agent").unwrap();
    (temp, codex)
}

#[test]
fn private_static_hook_is_owned_trusted_bounded_and_idempotent() {
    let (temp, codex) = prepared();
    assert_eq!(
        ensure_task_recovery(temp.path(), "agent").unwrap(),
        Registration::Updated
    );
    let record = recovery_registration(&codex, "agent").unwrap();
    let config = std::fs::read_to_string(codex.join("config.toml")).unwrap();
    let parsed: DocumentMut = config.parse().unwrap();
    assert_eq!(
        parsed["mcp_servers"][SERVER]["tools"]["read_task_context"]["output_token_limit"]
            .as_integer(),
        Some(2048)
    );
    assert_eq!(
        parsed["hooks"]["state"][&record.hook_key]["trusted_hash"].as_str(),
        Some(record.hook_hash.as_str())
    );
    assert!(parsed.get("approval_policy").is_none());
    let hook = std::fs::read_to_string(codex.join("hooks.json")).unwrap();
    assert!(owns_hook(&record, &hook));
    assert_eq!(
        ensure_task_recovery(temp.path(), "agent").unwrap(),
        Registration::Unchanged
    );
    assert_eq!(
        std::fs::read_to_string(codex.join("config.toml")).unwrap(),
        config
    );
    assert_eq!(
        std::fs::read_to_string(codex.join("hooks.json")).unwrap(),
        hook
    );
}

#[test]
fn user_file_collision_lower_limit_and_disable_preserve_all_user_bytes() {
    for case in ["file", "lower", "disabled"] {
        let (temp, codex) = prepared();
        if case == "file" {
            std::fs::write(codex.join("hooks.json"), b"{\"hooks\":{\"SessionStart\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"user-hook\"}]}]}}").unwrap();
        }
        if case == "lower" {
            let mut config: DocumentMut = std::fs::read_to_string(codex.join("config.toml"))
                .unwrap()
                .parse()
                .unwrap();
            config["mcp_servers"][SERVER]["tools"]["read_task_context"]["output_token_limit"] =
                value(32);
            std::fs::write(codex.join("config.toml"), config.to_string()).unwrap();
        }
        if case == "disabled" {
            let mut config: DocumentMut = std::fs::read_to_string(codex.join("config.toml"))
                .unwrap()
                .parse()
                .unwrap();
            config["features"]["codex_hooks"] = value(false);
            std::fs::write(codex.join("config.toml"), config.to_string()).unwrap();
        }
        let before = std::fs::read(codex.join("config.toml")).unwrap();
        let hook = std::fs::read(codex.join("hooks.json")).ok();
        assert!(matches!(
            ensure_task_recovery(temp.path(), "agent").unwrap(),
            Registration::Unavailable(_)
        ));
        assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), before);
        assert_eq!(std::fs::read(codex.join("hooks.json")).ok(), hook);
        assert!(!codex.join(RECOVERY_RECORD).exists());
    }
}

#[test]
fn edited_or_deleted_owned_hook_and_user_trust_changes_are_never_adopted() {
    for case in ["extra_handler", "deleted", "trust", "disabled"] {
        let (temp, codex) = prepared();
        ensure_task_recovery(temp.path(), "agent").unwrap();
        let record = recovery_registration(&codex, "agent").unwrap();
        match case {
            "extra_handler" => {
                let mut hook: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(codex.join("hooks.json")).unwrap(),
                )
                .unwrap();
                hook["hooks"]["SessionStart"][0]["hooks"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"type":"command","command":"third-party"}));
                std::fs::write(codex.join("hooks.json"), hook.to_string()).unwrap();
            }
            "deleted" => std::fs::remove_file(codex.join("hooks.json")).unwrap(),
            _ => {
                let mut config: DocumentMut = std::fs::read_to_string(codex.join("config.toml"))
                    .unwrap()
                    .parse()
                    .unwrap();
                if case == "trust" {
                    config["hooks"]["state"][&record.hook_key]["trusted_hash"] =
                        value("user-decision");
                } else {
                    config["hooks"]["state"][&record.hook_key]["enabled"] = value(false);
                }
                std::fs::write(codex.join("config.toml"), config.to_string()).unwrap();
            }
        }
        let before = std::fs::read(codex.join("config.toml")).unwrap();
        let hook = std::fs::read(codex.join("hooks.json")).ok();
        assert!(matches!(
            ensure_task_recovery(temp.path(), "agent").unwrap(),
            Registration::Unavailable(_)
        ));
        assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), before);
        assert_eq!(std::fs::read(codex.join("hooks.json")).ok(), hook);
    }
}

#[test]
fn pinned_normalized_hook_hash_matches_independent_canonical_golden() {
    let (_, hash) = hook_document("\"/bundle/wardian-cli\" mcp recovery-pointer");
    assert_eq!(
        hash,
        "sha256:8f1231edb965bb9a4d95d3c9657f475f6f65d40fd6e951688080fd6c0cd73af8"
    );
}

#[test]
fn ordinary_global_reconciliation_preserves_private_hook_state_and_budget() {
    let (temp, codex) = prepared();
    ensure_task_recovery(temp.path(), "agent").unwrap();
    let record = recovery_registration(&codex, "agent").unwrap();
    let native = temp.path().join("native");
    std::fs::create_dir_all(&native).unwrap();
    let global = "[hooks.state.user-handler]\nenabled = false\n[mcp_servers.wardian]\ncommand = 'user-global'\n";
    std::fs::write(native.join("config.toml"), global).unwrap();
    crate::utils::fs::sync_codex_agent_home(&native, &codex, Path::new("")).unwrap();
    let config: DocumentMut = std::fs::read_to_string(codex.join("config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        config["hooks"]["state"][&record.hook_key]["trusted_hash"].as_str(),
        Some(record.hook_hash.as_str())
    );
    assert_eq!(
        config["hooks"]["state"]["user-handler"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(
        config["mcp_servers"][SERVER]["tools"]["read_task_context"]["output_token_limit"]
            .as_integer(),
        Some(2048)
    );
    assert!(recovery_registration(&codex, "agent").is_ok());
    assert_eq!(
        ensure_task_recovery(temp.path(), "agent").unwrap(),
        Registration::Unchanged
    );
    assert_eq!(
        std::fs::read_to_string(native.join("config.toml")).unwrap(),
        global
    );

    let hooks = std::fs::read(codex.join("hooks.json")).unwrap();
    let ownership = std::fs::read(codex.join(RECOVERY_RECORD)).unwrap();
    let mut edited = config;
    edited["hooks"]["state"]["user-handler"]["enabled"] = value(true);
    edited["hooks"]["state"][&record.hook_key]["enabled"] = value(false);
    edited["hooks"]["state"][&record.hook_key]["trusted_hash"] = value("user-edited-trust");
    edited["mcp_servers"][SERVER]["tools"]["read_task_context"]["output_token_limit"] = value(1024);
    std::fs::write(codex.join("config.toml"), edited.to_string()).unwrap();
    crate::utils::fs::sync_codex_agent_home(&native, &codex, Path::new("")).unwrap();
    let preserved = std::fs::read_to_string(codex.join("config.toml")).unwrap();
    let config: DocumentMut = preserved.parse().unwrap();
    assert_eq!(
        config["hooks"]["state"]["user-handler"]["enabled"].as_bool(),
        Some(true)
    );
    assert_eq!(
        config["hooks"]["state"][&record.hook_key]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(
        config["hooks"]["state"][&record.hook_key]["trusted_hash"].as_str(),
        Some("user-edited-trust")
    );
    assert_eq!(
        config["mcp_servers"][SERVER]["tools"]["read_task_context"]["output_token_limit"]
            .as_integer(),
        Some(1024)
    );
    assert!(matches!(
        ensure_task_recovery(temp.path(), "agent").unwrap(),
        Registration::Unavailable(_)
    ));
    assert_eq!(
        std::fs::read_to_string(codex.join("config.toml")).unwrap(),
        preserved
    );
    assert_eq!(std::fs::read(codex.join("hooks.json")).unwrap(), hooks);
    assert_eq!(
        std::fs::read(codex.join(RECOVERY_RECORD)).unwrap(),
        ownership
    );
    assert_eq!(
        std::fs::read_to_string(native.join("config.toml")).unwrap(),
        global
    );
}
