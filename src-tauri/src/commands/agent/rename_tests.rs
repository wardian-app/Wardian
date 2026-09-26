use super::super::tests::{make_test_agent, WardianHomeGuard};
use super::super::{rename_agent, update_agent_config};
use crate::state::AppState;
use tauri::Manager;
use wardian_core::models::AgentConfig;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_does_not_hold_agent_map_while_waiting_for_roster_barrier() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp wardian home");
    unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
    let _home = WardianHomeGuard;
    wardian_core::db::init_db_at_path(&temp.path().join("state.db")).expect("init test database");

    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = "agent-1".to_string();
        config.session_name = "Alpha".to_string();
        config.folder = temp.path().to_string_lossy().replace('\\', "/");
    }
    state
        .agents
        .lock()
        .await
        .insert("agent-1".to_string(), agent);
    state.agent_order.lock().await.push("agent-1".to_string());

    let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .expect("acquire roster barrier")
        .expect("roster barrier available");
    let lifecycle_lock = state.agent_lifecycle_lock_for("agent-1").await;
    let app_handle = app.handle().clone();
    let rename = tokio::spawn(async move {
        let state = app_handle.state::<AppState>();
        rename_agent(
            "agent-1".to_string(),
            "Alpha-renamed".to_string(),
            state,
            app_handle.clone(),
        )
        .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if lifecycle_lock.try_lock().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rename acquires its lifecycle lock before waiting for the roster barrier");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let agent_map_available =
        tokio::time::timeout(std::time::Duration::from_millis(200), state.agents.lock())
            .await
            .is_ok();
    drop(barrier);
    let rename_result = tokio::time::timeout(std::time::Duration::from_secs(5), rename)
        .await
        .expect("rename completes once roster barrier is released")
        .expect("rename task joins");
    assert!(agent_map_available, "rename must not block configuration save behind the agent map while waiting for the roster barrier");
    rename_result.expect("valid rename succeeds");
    let persisted: Vec<AgentConfig> = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("settings").join("state.json"))
            .expect("read persisted agent configuration"),
    )
    .expect("parse persisted agent configuration");
    assert_eq!(persisted[0].session_name, "Alpha-renamed");
    let metadata = wardian_core::db::get_all_agents()
        .expect("load agent metadata")
        .into_iter()
        .find(|agent| agent.session_id == "agent-1")
        .expect("renamed agent metadata");
    assert_eq!(metadata.session_name, "Alpha-renamed");

    let mut config = state
        .agents
        .lock()
        .await
        .get("agent-1")
        .expect("renamed agent")
        .config
        .lock()
        .unwrap()
        .clone();
    config.session_name = "Beta".to_string();
    update_agent_config(config, state, app.handle().clone())
        .await
        .expect("configuration rename succeeds");
    let persisted: Vec<AgentConfig> = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("settings").join("state.json"))
            .expect("read updated agent configuration"),
    )
    .expect("parse updated agent configuration");
    assert_eq!(persisted[0].session_name, "Beta");
    let metadata = wardian_core::db::get_all_agents()
        .expect("load updated agent metadata")
        .into_iter()
        .find(|agent| agent.session_id == "agent-1")
        .expect("updated agent metadata");
    assert_eq!(metadata.session_name, "Beta");
}
