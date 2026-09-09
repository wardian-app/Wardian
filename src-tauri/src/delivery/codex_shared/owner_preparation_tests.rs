//! Filesystem fixtures for the gated owner's preprojection recovery boundary.
use super::prepare_owner_habitat;
use crate::delivery::codex_shared::launch_config::prepare_launch_config;
use crate::utils::fs::{habitat_codex_home, prepare_habitat_workspace, prepare_provider_habitat};
use crate::utils::{codex_home::TEST_ROOTS, codex_messaging::TEST_NATIVE_HOME};
use std::path::PathBuf;

const JOURNAL: &str = ".wardian-launch-config.json";
const GLOBAL: &str = "model = 'current-global'\n[mcp_servers.fixture]\ncommand = 'inert'\n";

struct Fixture {
    _temp: tempfile::TempDir,
    workspace: PathBuf,
    old_home: Option<std::ffi::OsString>,
    old_source: Option<PathBuf>,
    old_roots: Option<Vec<PathBuf>>,
}

impl Fixture {
    fn new() -> Self {
        #[cfg(target_os = "macos")]
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        #[cfg(not(target_os = "macos"))]
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("wardian");
        let native = temp.path().join("native");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::create_dir_all(&native).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(native.join("config.toml"), GLOBAL).unwrap();
        std::fs::write(
            home.join("bin")
                .join(crate::utils::cli_install::bundled_cli_file_name()),
            b"inert CLI fixture; never executed",
        )
        .unwrap();
        let old_home = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", &home);
        let old_source = TEST_NATIVE_HOME.with(|source| source.replace(Some(native)));
        let old_roots = TEST_ROOTS.with(|roots| roots.replace(Some(vec![temp.path().join("c")])));
        Self {
            _temp: temp,
            workspace,
            old_home,
            old_source,
            old_roots,
        }
    }

    fn neutral(&self) -> PathBuf {
        prepare_habitat_workspace(&self.workspace, "", "agent").unwrap()
    }

    fn config(home: &std::path::Path) -> toml_edit::DocumentMut {
        std::fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        TEST_NATIVE_HOME.with(|source| source.replace(self.old_source.take()));
        TEST_ROOTS.with(|roots| roots.replace(self.old_roots.take()));
        match self.old_home.take() {
            Some(home) => std::env::set_var("WARDIAN_HOME", home),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

fn overlay_args() -> Vec<String> {
    [
        "app-server",
        "-c",
        "model='interrupted'",
        "-c",
        "web_search='live'",
        "-c",
        "sandbox_workspace_write.writable_roots=['old-permission']",
        "-c",
        "developer_instructions='old memory context'",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[test]
fn owner_recovers_crashed_overlay_before_first_config_and_mcp_projection() {
    let _lock = crate::utils::wardian_test_env_lock();
    let fixture = Fixture::new();
    let habitat = fixture.neutral();
    let home = habitat_codex_home(&habitat);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        "# retained\npersonality='friendly'\n",
    )
    .unwrap();
    let overlay = prepare_launch_config(&home, &overlay_args()).unwrap();
    std::mem::forget(overlay); // Simulate process loss after config publication.
    let pending_config = std::fs::read(home.join("config.toml")).unwrap();
    let pending_journal = std::fs::read(home.join(JOURNAL)).unwrap();

    // Exercise the same neutral preparation used by manager before owner creation.
    assert_eq!(fixture.neutral(), habitat);
    assert_eq!(
        std::fs::read(home.join("config.toml")).unwrap(),
        pending_config
    );
    assert_eq!(std::fs::read(home.join(JOURNAL)).unwrap(), pending_journal);
    assert!(Fixture::config(&home).get("mcp_servers").is_none());

    let (_, prepared_home) = prepare_owner_habitat(&fixture.workspace, "", "agent").unwrap();
    let restored = Fixture::config(&prepared_home);
    // Reversing recovery/projection would leave model absent instead of importing
    // the current default: reconciliation preserves the old applied local model.
    assert_eq!(restored["model"].as_str(), Some("current-global"));
    assert_eq!(restored["personality"].as_str(), Some("friendly"));
    assert!(restored.to_string().contains("# retained"));
    assert!(restored.get("web_search").is_none());
    assert!(restored.get("developer_instructions").is_none());
    assert!(restored["sandbox_workspace_write"]
        .get("writable_roots")
        .is_none());
    assert_eq!(
        restored["mcp_servers"]["fixture"]["command"].as_str(),
        Some("inert")
    );
    assert_eq!(
        restored["mcp_servers"]["wardian"]["tool_timeout_sec"].as_integer(),
        Some(70)
    );
    assert!(!home.join(JOURNAL).exists());
}

#[test]
fn generic_refresh_does_not_recover_a_pending_live_overlay() {
    let _lock = crate::utils::wardian_test_env_lock();
    let fixture = Fixture::new();
    let (_, home) = prepare_owner_habitat(&fixture.workspace, "", "agent").unwrap();
    let mut overlay = prepare_launch_config(&home, &overlay_args()).unwrap();
    let pending_journal = std::fs::read(home.join(JOURNAL)).unwrap();
    prepare_provider_habitat("codex", &fixture.workspace, "", Some("agent")).unwrap();
    let pending = Fixture::config(&home);
    assert_eq!(pending["model"].as_str(), Some("interrupted"));
    assert_eq!(pending["web_search"].as_str(), Some("live"));
    assert_eq!(std::fs::read(home.join(JOURNAL)).unwrap(), pending_journal);
    overlay.restore().unwrap();
    assert_eq!(
        Fixture::config(&home)["model"].as_str(),
        Some("current-global")
    );
}

#[test]
fn owner_prepares_a_new_home_without_config_or_journal() {
    let _lock = crate::utils::wardian_test_env_lock();
    let fixture = Fixture::new();
    let habitat = fixture.neutral();
    assert!(!habitat_codex_home(&habitat).exists());
    let (_, home) = prepare_owner_habitat(&fixture.workspace, "", "agent").unwrap();
    assert_eq!(
        Fixture::config(&home)["model"].as_str(),
        Some("current-global")
    );
    assert!(home.join(".wardian-messaging.json").is_file());
    assert!(!home.join(JOURNAL).exists());
}

#[test]
fn invalid_recovery_input_blocks_config_and_mcp_projection_without_mutation() {
    let _lock = crate::utils::wardian_test_env_lock();
    let fixture = Fixture::new();
    let home = habitat_codex_home(&fixture.neutral());
    std::fs::create_dir_all(&home).unwrap();
    for (config, journal) in [("# unchanged\n", Some("not a journal")), ("model=[", None)] {
        std::fs::write(home.join("config.toml"), config).unwrap();
        if let Some(journal) = journal {
            std::fs::write(home.join(JOURNAL), journal).unwrap();
        }
        assert!(prepare_owner_habitat(&fixture.workspace, "", "agent").is_err());
        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            config
        );
        assert!(!home.join(".wardian-messaging.json").exists());
        if let Some(journal) = journal {
            assert_eq!(
                std::fs::read_to_string(home.join(JOURNAL)).unwrap(),
                journal
            );
            std::fs::remove_file(home.join(JOURNAL)).unwrap();
        }
    }
}

#[test]
fn owner_managed_instructions_reach_tui_overlay_independently_of_memory() {
    let _lock = crate::utils::wardian_test_env_lock();
    for memory_enabled in [false, true] {
        for background in [false, true] {
            let fixture = Fixture::new();
            let wardian_home = crate::utils::get_wardian_home().unwrap();
            let sources = [
                (wardian_home.join("common/AGENTS.md"), "COMMON_SENTINEL"),
                (
                    wardian_home.join("classes/Builder/AGENTS.md"),
                    "CLASS_SENTINEL",
                ),
                (
                    wardian_home.join("agents/agent/AGENTS.md"),
                    "AGENT_SENTINEL",
                ),
                (fixture.workspace.join("AGENTS.md"), "WORKSPACE_ONLY"),
            ];
            for (path, text) in &sources {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            std::fs::create_dir_all(wardian_home.join("settings")).unwrap();
            std::fs::write(
                wardian_home.join("settings/app.json"),
                serde_json::json!({"schema_version":2,"overrides":{"memory_enabled":memory_enabled}}).to_string(),
            ).unwrap();
            assert_eq!(crate::utils::memory_feature_enabled(), memory_enabled);
            let (habitat, home) =
                prepare_owner_habitat(&fixture.workspace, "Builder", "agent").unwrap();
            std::fs::write(habitat.join("AGENTS.md"), "UNTRUSTED_GENERATED_FILE").unwrap();
            let config_before = std::fs::read(home.join("config.toml")).unwrap();
            let spec = crate::delivery::native_broker::NativeSessionSpec {
                target_agent_id: "agent".into(),
                provider: "codex".into(),
                generation: 7,
                workspace: fixture.workspace.clone(),
                config: wardian_core::models::AgentConfig {
                    session_id: "agent".into(),
                    provider: "codex".into(),
                    agent_class: "Builder".into(),
                    is_off: background,
                    ..Default::default()
                },
            };
            let original = vec![
                "app-server".to_owned(),
                "-c".into(),
                "model='fixture-model'".into(),
                "-c".into(),
                "model_reasoning_effort='low'".into(),
            ];
            let mut args = original.clone();
            super::append_runtime_context(&mut args, &spec, &habitat, &home).unwrap();
            let overrides: Vec<_> = args
                .iter()
                .filter(|arg| arg.starts_with("developer_instructions="))
                .collect();
            assert_eq!(overrides.len(), 1);
            let parsed = overrides[0].parse::<toml_edit::DocumentMut>().unwrap();
            let text = parsed["developer_instructions"].as_str().unwrap();
            assert!(text.find("COMMON_SENTINEL").unwrap() < text.find("CLASS_SENTINEL").unwrap());
            assert!(text.find("CLASS_SENTINEL").unwrap() < text.find("AGENT_SENTINEL").unwrap());
            assert!(!text.contains("WORKSPACE_ONLY"));
            assert!(!text.contains("UNTRUSTED_GENERATED_FILE"));
            assert_eq!(
                text.matches("## Wardian memory").count(),
                usize::from(memory_enabled)
            );
            if !background {
                // This is the actual file handoff consumed by an ordinary TUI;
                // no provider process or alternate remote mode is involved.
                let mut overlay = prepare_launch_config(&home, &args).unwrap();
                assert_eq!(
                    Fixture::config(&home)["developer_instructions"].as_str(),
                    Some(text)
                );
                overlay.restore().unwrap();
            }
            let config_after = std::fs::read(home.join("config.toml")).unwrap();
            if background {
                assert_eq!(config_after, config_before);
            } else {
                // The journal restores leaf values; TOML quote style may be
                // normalized for a temporarily overridden leaf.
                let mut expected = std::str::from_utf8(&config_before)
                    .unwrap()
                    .parse::<toml_edit::DocumentMut>()
                    .unwrap();
                expected["model"] = toml_edit::value("current-global");
                assert_eq!(
                    String::from_utf8(config_after).unwrap(),
                    expected.to_string()
                );
                assert!(!home.join(JOURNAL).exists());
            }
            let index = args
                .iter()
                .position(|arg| arg.starts_with("developer_instructions="))
                .unwrap();
            args.drain(index - 1..=index);
            assert_eq!(args, original);
            for (path, expected) in &sources {
                assert_eq!(std::fs::read_to_string(path).unwrap(), *expected);
            }
        }
    }
}

#[test]
fn owner_memory_failures_block_context_publication_before_launch() {
    let _lock = crate::utils::wardian_test_env_lock();
    for failure in ["initialize", "compile"] {
        let fixture = Fixture::new();
        let wardian_home = crate::utils::get_wardian_home().unwrap();
        std::fs::create_dir_all(wardian_home.join("settings")).unwrap();
        std::fs::write(
            wardian_home.join("settings/app.json"),
            r#"{"schema_version":2,"overrides":{"memory_enabled":true}}"#,
        )
        .unwrap();
        assert!(crate::utils::memory_feature_enabled());
        let (habitat, home) = prepare_owner_habitat(&fixture.workspace, "", "agent").unwrap();
        std::fs::write(habitat.join("AGENTS.md"), "UNCHANGED_CONTEXT").unwrap();
        let config_before = std::fs::read(home.join("config.toml")).unwrap();
        let memory_path = wardian_core::paths::memory_db_path().unwrap();
        if failure == "initialize" {
            std::fs::write(&memory_path, b"not a SQLite database").unwrap();
        } else {
            let store = wardian_core::memory::MemoryStore::from_default_home().unwrap();
            let connection = rusqlite::Connection::open(store.path()).unwrap();
            // SQLite permits text in this INTEGER column. Opening and migrating
            // succeeds, but materializing the active record for the brief fails.
            connection
                .execute_batch(
                    "INSERT INTO memory_records
                 (revision_id,memory_id,revision,agent_id,kind,text,evidence_excerpt,
                  evidence_hash,status,created_at,updated_at,last_verified_at)
                 VALUES ('revision','memory','invalid-integer','agent','stable','note',
                         'fixture','hash','active','now','now','now');",
                )
                .unwrap();
            drop(connection);
            assert!(wardian_core::memory::MemoryStore::from_default_home().is_ok());
        }
        let spec = crate::delivery::native_broker::NativeSessionSpec {
            target_agent_id: "agent".into(),
            provider: "codex".into(),
            generation: 7,
            workspace: fixture.workspace.clone(),
            config: wardian_core::models::AgentConfig {
                session_id: "agent".into(),
                provider: "codex".into(),
                ..Default::default()
            },
        };
        let original = vec!["app-server".to_owned()];
        let mut args = original.clone();
        let error = super::append_runtime_context(&mut args, &spec, &habitat, &home)
            .expect_err("memory failures must stop the owner's pre-spawn setup");
        assert!(error.message.contains(failure), "{error:?}");
        assert_eq!(args, original);
        assert_eq!(
            std::fs::read(home.join("config.toml")).unwrap(),
            config_before
        );
        assert_eq!(
            std::fs::read_to_string(habitat.join("AGENTS.md")).unwrap(),
            "UNCHANGED_CONTEXT"
        );
        assert!(!home.join(JOURNAL).exists());
    }
}
