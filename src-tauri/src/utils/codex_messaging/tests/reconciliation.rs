use super::*;
use crate::utils::fs::{prepare_provider_habitat, sync_codex_agent_home};

struct PreparationContext(Option<std::ffi::OsString>);

impl PreparationContext {
    fn set(home: &Path, native: &Path) -> Self {
        let old = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", home);
        TEST_NATIVE_HOME.with(|value| *value.borrow_mut() = Some(native.to_owned()));
        Self(old)
    }
}

impl Drop for PreparationContext {
    fn drop(&mut self) {
        TEST_NATIVE_HOME.with(|value| *value.borrow_mut() = None);
        match self.0.take() {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

#[test]
fn actual_sync_preserves_local_wardian_while_other_provider_entries_refresh_and_prune() {
    let temp = tempfile::tempdir().unwrap();
    let native = temp.path().join("native");
    std::fs::create_dir_all(&native).unwrap();
    for inline in [false, true] {
        let codex = prepare(temp.path(), if inline { "inline" } else { "table" }, true);
        let native_command = native.join("user-server").to_string_lossy().into_owned();
        let mut entry = Table::new();
        entry["command"] = value(&native_command);
        entry["enabled"] = value(false);
        entry["tools"] = Item::Table(Table::new());
        entry["tools"]["send_message"] = Item::Table(Table::new());
        entry["tools"]["send_message"]["approval_mode"] = value("prompt");
        let entry = if inline {
            Item::Value(entry.into_inline_table().into())
        } else {
            Item::Table(entry)
        };
        let mut local = DocumentMut::new();
        local["mcp_servers"] = Item::Table(Table::new());
        local["mcp_servers"][SERVER] = entry;
        for (name, command) in [("shared", "old"), ("stale", "cua_node stale-runtime")] {
            local["mcp_servers"][name] = Item::Table(Table::new());
            local["mcp_servers"][name]["command"] = value(command);
        }
        if inline {
            let table = local
                .as_table_mut()
                .remove("mcp_servers")
                .unwrap()
                .into_table()
                .unwrap();
            local["mcp_servers"] = value(table.into_inline_table());
        }
        std::fs::write(codex.join("config.toml"), local.to_string()).unwrap();
        // Compare the parsed on-disk entry that sync actually receives. An
        // in-memory inline table has not acquired its serialized leading space.
        // Keep the exact entry assertion, including policy values/decoration.
        let expected = read_config(&codex)["mcp_servers"][SERVER].to_string();
        for cycle in 0..3 {
            let mut global = format!("[mcp_servers.shared]\ncommand = 'new-{cycle}'\n");
            if cycle != 1 {
                global.push_str(
                    "[mcp_servers.wardian]\ncommand = 'conflicting-global'\nenabled = true\n",
                );
            }
            std::fs::write(native.join("config.toml"), &global).unwrap();
            sync_codex_agent_home(&native, &codex, Path::new("")).unwrap();
            let result = read_config(&codex);
            assert_eq!(result["mcp_servers"][SERVER].to_string(), expected);
            assert_eq!(
                result["mcp_servers"]["shared"]["command"].as_str(),
                Some(format!("new-{cycle}").as_str())
            );
            assert!(result["mcp_servers"].get("stale").is_none());
            assert_eq!(
                std::fs::read_to_string(native.join("config.toml")).unwrap(),
                global
            );
        }
    }
}

#[test]
fn repeated_real_preparation_registers_then_preserves_owned_policy_and_user_edits() {
    let _guard = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("wardian");
    let native = temp.path().join("native");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _context = PreparationContext::set(&home, &native);
    let mut expected = Vec::new();
    for agent in ["default-agent", "owned-agent", "edited-agent"] {
        // Install only an inert CLI file; registration must happen inside
        // actual habitat preparation, without a separate user/helper command.
        prepare(&home, agent, true);
        let habitat = prepare_provider_habitat("codex", &workspace, "", Some(agent))
            .unwrap()
            .unwrap();
        let codex = habitat.join(".codex");
        let mut document = read_config(&codex);
        assert_eq!(
            document["mcp_servers"][SERVER]["env"]["WARDIAN_SESSION_ID"].as_str(),
            Some(agent)
        );
        assert!(codex.join(RECORD).is_file());
        let entry = &mut document["mcp_servers"][SERVER];
        assert_eq!(entry["tool_timeout_sec"].as_integer(), Some(70));
        if agent != "default-agent" {
            // Even a user value matching the old default is not ours to reset.
            entry["tool_timeout_sec"] = value(if agent == "owned-agent" { 660 } else { 999 });
        }
        entry["enabled"] = value(false);
        entry["tools"] = Item::Table(Table::new());
        entry["tools"]["followup_task"] = Item::Table(Table::new());
        entry["tools"]["followup_task"]["approval_mode"] = value("prompt");
        if agent == "edited-agent" {
            entry["command"] = value(native.join("user-selected").to_str().unwrap());
        }
        expected.push((
            agent,
            codex.clone(),
            entry.to_string(),
            std::fs::read(codex.join(RECORD)).unwrap(),
        ));
        std::fs::write(codex.join("config.toml"), document.to_string()).unwrap();
    }
    for cycle in 0..3 {
        let global = if cycle == 1 {
            "[mcp_servers.other]\ncommand = 'cua_node current-runtime'\n"
        } else {
            "[mcp_servers.wardian]\ncommand = 'global-other-server'\nenabled = true\ntool_timeout_sec = 660\n"
        };
        std::fs::write(native.join("config.toml"), global).unwrap();
        for (agent, codex, expected_entry, record) in &expected {
            // Exercise explicit sync as well as the normal preparation path,
            // whose internal sync must not undo the local receiver identity.
            sync_codex_agent_home(&native, codex, Path::new("")).unwrap();
            prepare_provider_habitat("codex", &workspace, "", Some(agent)).unwrap();
            assert_eq!(
                read_config(codex)["mcp_servers"][SERVER].to_string(),
                *expected_entry
            );
            assert_eq!(std::fs::read(codex.join(RECORD)).unwrap(), *record);
        }
        assert_eq!(
            std::fs::read_to_string(native.join("config.toml")).unwrap(),
            global
        );
    }
}

#[test]
fn absent_local_entry_inherits_global_and_real_preparation_preserves_the_collision() {
    let _guard = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("wardian");
    let native = temp.path().join("native");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let global = "[mcp_servers.wardian]\ncommand = 'user-global-server'\nenabled = false\n";
    std::fs::write(native.join("config.toml"), global).unwrap();
    let codex = prepare(&home, "agent", true);
    let _context = PreparationContext::set(&home, &native);
    for _ in 0..3 {
        prepare_provider_habitat("codex", &workspace, "", Some("agent")).unwrap();
        let config = read_config(&codex);
        assert_eq!(
            config["mcp_servers"][SERVER]["command"].as_str(),
            Some("user-global-server")
        );
        assert_eq!(
            config["mcp_servers"][SERVER]["enabled"].as_bool(),
            Some(false)
        );
        assert!(
            !codex.join(RECORD).exists(),
            "inherited user server must not be adopted"
        );
        assert_eq!(
            std::fs::read_to_string(native.join("config.toml")).unwrap(),
            global
        );
    }
}
