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
