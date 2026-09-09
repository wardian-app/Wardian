//! Process-free removal and failed-clone retention regressions.
use super::super::tests::{make_test_agent, WardianHomeGuard};
use super::super::{clone_cleanup_created_profile_dirs, persist_agent_config, remove_agent};
use crate::state::AppState;
use crate::utils::fs::create_directory_link;
use tauri::Manager;
use wardian_core::models::AgentConfig;

#[test]
fn failed_clone_cleanup_retains_compact_mapping_and_backup_without_join_proof() {
    let temp = tempfile::tempdir().expect("temp dir");
    let profile = temp.path().join("agents/failed-clone");
    let backup = profile.join("habitat/.codex-precompact-test");
    std::fs::create_dir_all(&backup).expect("backup");
    let mapping = profile.join(".wardian-codex-home.json");
    std::fs::write(&mapping, "unverified ownership").expect("mapping");
    std::fs::write(backup.join("session"), "retain").expect("backup data");

    clone_cleanup_created_profile_dirs(std::slice::from_ref(&profile));

    assert_eq!(
        std::fs::read_to_string(mapping).unwrap(),
        "unverified ownership"
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("session")).unwrap(),
        "retain"
    );
}

#[tokio::test]
async fn removal_persistence_failure_retains_roster_metadata_and_profile() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp wardian home");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let mut agent = make_test_agent();
    agent.runtime_generation = Some(7); // No real provider is started.
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = "agent-delete-failure".into();
        config.session_name = "DeleteFailure".into();
        persist_agent_config(&config, None).unwrap();
    }
    state
        .agents
        .lock()
        .await
        .insert("agent-delete-failure".into(), agent);
    state
        .agent_order
        .lock()
        .await
        .push("agent-delete-failure".into());
    let profile = temp.path().join("agents/agent-delete-failure");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("AGENTS.md"), "retain").unwrap();
    std::fs::create_dir_all(temp.path().join("settings/state.json")).unwrap();

    let error = remove_agent(
        "agent-delete-failure".into(),
        Some("DeleteFailure"),
        state,
        app.handle().clone(),
        false,
    )
    .await
    .expect_err("persistence failure must abort durable deletion");

    assert!(
        error.contains("Failed to persist agent deletion"),
        "{error}"
    );
    let state = app.state::<AppState>();
    assert_eq!(
        state.agents.lock().await["agent-delete-failure"].runtime_generation,
        Some(7)
    );
    assert_eq!(
        state.agent_order.lock().await.as_slice(),
        ["agent-delete-failure"]
    );
    assert!(wardian_core::db::get_all_agents()
        .unwrap()
        .iter()
        .any(|agent| agent.session_id == "agent-delete-failure"));
    assert_eq!(
        std::fs::read_to_string(profile.join("AGENTS.md")).unwrap(),
        "retain"
    );
}

#[tokio::test]
async fn removal_compact_failure_is_best_effort_after_durable_commit() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp wardian home");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = "agent-delete-retain".into();
        config.session_name = "DeleteRetain".into();
        persist_agent_config(&config, None).unwrap();
    }
    state
        .agents
        .lock()
        .await
        .insert("agent-delete-retain".into(), agent);
    state
        .agent_order
        .lock()
        .await
        .push("agent-delete-retain".into());
    let profile = temp.path().join("agents/agent-delete-retain");
    let backup = profile.join("habitat/.codex-precompact-test");
    std::fs::create_dir_all(&backup).unwrap();
    let mapping = profile.join(".wardian-codex-home.json");
    std::fs::write(&mapping, "unverified owner").unwrap();
    std::fs::write(backup.join("session"), "retain").unwrap();

    remove_agent(
        "agent-delete-retain".into(),
        Some("DeleteRetain"),
        state,
        app.handle().clone(),
        false,
    )
    .await
    .expect("postcommit compact cleanup failure is best effort");

    let state = app.state::<AppState>();
    assert!(!state
        .agents
        .lock()
        .await
        .contains_key("agent-delete-retain"));
    assert!(state.agent_order.lock().await.is_empty());
    assert!(!wardian_core::db::get_all_agents()
        .unwrap()
        .iter()
        .any(|agent| agent.session_id == "agent-delete-retain"));
    let persisted: Vec<AgentConfig> =
        serde_json::from_slice(&std::fs::read(temp.path().join("settings/state.json")).unwrap())
            .unwrap();
    assert!(persisted.is_empty());
    assert_eq!(
        std::fs::read_to_string(mapping).unwrap(),
        "unverified owner"
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("session")).unwrap(),
        "retain"
    );
}

#[tokio::test]
async fn removal_process_join_requires_observed_exit() {
    // A successful kill request is insufficient while the child is running.
    let error = super::wait_for_removal_process_exit(std::time::Duration::ZERO, || Ok(false))
        .await
        .expect_err("running child must retain ownership");
    assert!(error.contains("Timed out joining"));
    super::wait_for_removal_process_exit(std::time::Duration::ZERO, || Ok(true))
        .await
        .expect("observed exit is sufficient even at the deadline");
    let error = super::wait_for_removal_process_exit(std::time::Duration::ZERO, || {
        Err("wait failed".to_string())
    })
    .await
    .expect_err("failed wait is not an exit");
    assert_eq!(error, "wait failed");
}

#[tokio::test]
async fn removal_process_join_rejects_missing_tui_handle() {
    let mut agent = make_test_agent();
    agent.runtime_generation = Some(7);
    // No real PID or provider process; broker state alone cannot prove exit.
    let error = super::join_agent_processes_for_removal(&mut agent)
        .await
        .expect_err("missing TUI handle must retain ownership records");
    assert!(error.contains("process handle is missing"));
    agent.runtime_generation = None;
    super::join_agent_processes_for_removal(&mut agent)
        .await
        .expect("an agent with no processes needs no join");
}

#[test]
fn removal_join_failure_retains_mapping_before_preparation() {
    let temp = tempfile::tempdir().expect("temp dir");
    let profile = temp.path().join("agents/agent-retained");
    std::fs::create_dir_all(&profile).unwrap();
    let mapping = profile.join(".wardian-codex-home.json");
    std::fs::write(&mapping, "retain").unwrap();
    let error = super::cleanup_removed_agent_directory(
        temp.path(),
        "agent-retained",
        Err("TUI join failed".into()),
    )
    .expect_err("join failure must precede filesystem cleanup");
    assert_eq!(error, "TUI join failed");
    assert_eq!(std::fs::read_to_string(mapping).unwrap(), "retain");
    assert!(!temp.path().join("locks").exists());
}

#[test]
fn removal_compact_cleanup_failure_retains_mapping_and_backup() {
    let temp = tempfile::tempdir().expect("temp dir");
    let profile = temp.path().join("agents/agent-retained");
    let backup = profile.join("habitat/.codex-precompact-test");
    std::fs::create_dir_all(&backup).unwrap();
    let mapping = profile.join(".wardian-codex-home.json");
    std::fs::write(&mapping, "invalid ownership record").unwrap();
    std::fs::write(backup.join("session"), "retain").unwrap();
    let error = super::cleanup_removed_agent_directory(temp.path(), "agent-retained", Ok(()))
        .expect_err("unverified compact ownership must retain the agent directory");
    assert!(!error.is_empty());
    assert_eq!(
        std::fs::read_to_string(mapping).unwrap(),
        "invalid ownership record"
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("session")).unwrap(),
        "retain"
    );
}

#[test]
fn removal_preparation_lock_blocks_cleanup_and_survives_agent_deletion() {
    let temp = tempfile::tempdir().expect("temp dir");
    let profile = temp.path().join("agents/agent-locked");
    std::fs::create_dir_all(profile.join("habitat/.codex")).unwrap();
    std::fs::write(profile.join("AGENTS.md"), "retain").unwrap();
    let guard = crate::utils::codex_home::acquire_preparation(temp.path(), "agent-locked")
        .expect("preparation lock");
    let error = super::cleanup_removed_agent_directory(temp.path(), "agent-locked", Ok(()))
        .expect_err("concurrent preparation must block cleanup");
    assert!(error.contains("busy"), "{error}");
    assert!(profile.join("AGENTS.md").is_file());
    drop(guard);
    super::cleanup_removed_agent_directory(temp.path(), "agent-locked", Ok(()))
        .expect("joined ordinary home is removed after preparation ends");
    assert!(!profile.exists());
    assert_eq!(
        std::fs::read_dir(temp.path().join("locks"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn removal_unmaterialized_agent_directory_is_removed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let profile = temp.path().join("agents/agent-off");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("AGENTS.md"), "profile only").unwrap();
    super::cleanup_removed_agent_directory(temp.path(), "agent-off", Ok(()))
        .expect("Off agent without a Codex home can be deleted");
    assert!(!profile.exists());
}

#[test]
fn removal_does_not_follow_nested_shared_session_links() {
    let temp = tempfile::tempdir().expect("temp dir");
    let profile = temp.path().join("agents/agent-links");
    let codex = profile.join("habitat/.codex");
    let shared = temp.path().join("shared-sessions");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::create_dir(&shared).unwrap();
    std::fs::write(shared.join("session"), "shared").unwrap();
    create_directory_link(&shared, &codex.join("sessions")).expect("shared session link");
    super::cleanup_removed_agent_directory(temp.path(), "agent-links", Ok(()))
        .expect("plain home with shared session link can be removed");
    assert!(!profile.exists());
    assert_eq!(
        std::fs::read_to_string(shared.join("session")).unwrap(),
        "shared"
    );
}
