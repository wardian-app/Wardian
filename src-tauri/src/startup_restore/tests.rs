use super::*;
use crate::commands::agent::{list_agents, update_agent_config};
use std::future::{poll_fn, Future};
use std::task::Poll;
use tauri::Manager;
use wardian_core::models::{AgentConfig, AgentSessionPersistenceOverride, ProviderConfig};

struct TestHome {
    previous: Option<std::ffi::OsString>,
    directory: tempfile::TempDir,
}

impl TestHome {
    fn new() -> Self {
        let home = Self {
            previous: std::env::var_os("WARDIAN_HOME"),
            directory: tempfile::tempdir().unwrap(),
        };
        unsafe { std::env::set_var("WARDIAN_HOME", home.directory.path()) };
        wardian_core::db::init_db_at_path(&home.directory.path().join("state.db")).unwrap();
        home
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => unsafe { std::env::set_var("WARDIAN_HOME", previous) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }
    }
}

#[tokio::test]
async fn acknowledged_config_survives_paused_and_live_restore_publication() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let home = TestHome::new();
    for (is_off, final_status) in [(true, "Off"), (false, "Idle"), (false, "Error")] {
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            session_name: format!("Restored-{final_status}"),
            provider: "claude".into(),
            provider_config: ProviderConfig::Claude(Default::default()),
            folder: home.directory.path().to_string_lossy().into_owned(),
            session_persistence: AgentSessionPersistenceOverride::Resume,
            resume_session: Some(uuid::Uuid::new_v4().to_string()),
            is_off,
            ..Default::default()
        };
        let publication = RestorePublication::begin(&state, &config.session_id)
            .await
            .unwrap();
        publication
            .publish(
                &state,
                crate::restored_agent_without_process(
                    config.clone(),
                    "Restoring",
                    String::new(),
                    None,
                    None,
                ),
            )
            .await;

        // This is the same publication object carried by lib.rs through the
        // deferred provider startup. Hold completion at a deterministic barrier.
        let (release, gate) = tokio::sync::oneshot::channel();
        let handle = app.handle().clone();
        let captured = config.clone();
        let restoration = tokio::spawn(async move {
            gate.await.unwrap();
            let mut completed = crate::restored_agent_without_process(
                captured,
                final_status,
                String::new(),
                None,
                None,
            );
            completed.runtime_generation = Some(42);
            publication
                .publish(&handle.state::<AppState>(), completed)
                .await;
        });
        let mut edited = list_agents(app.state()).await.unwrap().pop().unwrap();
        edited.session_persistence = AgentSessionPersistenceOverride::Fresh;
        edited.description = "acknowledged edit".into();
        let mut update = std::pin::pin!(update_agent_config(
            edited,
            app.state(),
            app.handle().clone()
        ));
        // Poll the real IPC mutation to completion on base, or to its lifecycle
        // gate when fixed. Do not await it before releasing the restoring owner.
        let first_poll = poll_fn(|cx| Poll::Ready(update.as_mut().poll(cx))).await;
        release.send(()).unwrap();
        restoration.await.unwrap();
        match first_poll {
            Poll::Ready(result) => result.unwrap(),
            Poll::Pending => update.await.unwrap(),
        }
        persist_roster(&state).await.unwrap();
        let live = list_agents(app.state()).await.unwrap().pop().unwrap();
        assert_eq!(live.session_persistence, AgentSessionPersistenceOverride::Fresh,
            "late startup publication must preserve the acknowledged fresh setting (is_off={is_off})");
        assert_eq!(live.description, "acknowledged edit");
        {
            let agents = state.agents.lock().await;
            let completed = agents.get(&config.session_id).unwrap();
            assert_eq!(completed.runtime_generation, Some(42));
            assert_eq!(*completed.current_status.lock().unwrap(), final_status);
        }
        let disk: Vec<AgentConfig> = serde_json::from_slice(
            &std::fs::read(home.directory.path().join("settings/state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            disk[0].session_persistence,
            AgentSessionPersistenceOverride::Fresh
        );
        assert_eq!(disk[0].description, "acknowledged edit");

        // Both startup and ordinary resume use the same config preparation.
        let mut next_launch = live;
        next_launch.is_off = false;
        crate::commands::agent::prepare_restored_config_for_spawn(&mut next_launch).unwrap();
        assert!(next_launch.resume_session.is_none());
        assert!(next_launch.fresh_provider_session_id.is_some());
    }
}

#[tokio::test]
async fn claim_waits_before_selection_and_preserves_an_existing_owner() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let home = TestHome::new();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        session_name: "Already-current".into(),
        folder: home.directory.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    let current_owner = state.lock_agent_lifecycle(&config.session_id).await;
    let mut claim = std::pin::pin!(RestorePublication::begin(&state, &config.session_id));
    assert!(poll_fn(|cx| Poll::Ready(claim.as_mut().poll(cx)))
        .await
        .is_pending());
    // A prior lifecycle operation registers its final runtime before releasing
    // ownership. The waiting startup task must not reuse its older snapshot.
    let mut registered =
        crate::restored_agent_without_process(config.clone(), "Off", String::new(), None, None);
    registered.runtime_generation = Some(73);
    state
        .agents
        .lock()
        .await
        .insert(config.session_id.clone(), registered);
    state
        .agent_order
        .lock()
        .await
        .push(config.session_id.clone());
    drop(current_owner);
    let mut edited = config.clone();
    edited.session_persistence = AgentSessionPersistenceOverride::Fresh;
    // The queued claim must yield to the registered owner and release its gate.
    assert!(claim.await.is_none());
    update_agent_config(edited, app.state(), app.handle().clone())
        .await
        .unwrap();
    assert!(RestorePublication::begin(&state, &config.session_id)
        .await
        .is_none());
    assert_eq!(
        state.agents.lock().await[&config.session_id].runtime_generation,
        Some(73)
    );
    assert_eq!(
        list_agents(app.state()).await.unwrap()[0].session_persistence,
        AgentSessionPersistenceOverride::Fresh
    );
    persist_roster(&state).await.unwrap();
    let disk: Vec<AgentConfig> = serde_json::from_slice(
        &std::fs::read(home.directory.path().join("settings/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        disk[0].session_persistence,
        AgentSessionPersistenceOverride::Fresh
    );
}

#[tokio::test]
async fn restoration_keeps_global_and_other_agent_locks_available() {
    let state = AppState::new();
    let claim = RestorePublication::begin(&state, "restoring")
        .await
        .unwrap();
    assert!(state.agents.try_lock().is_ok());
    assert!(state.agent_order.try_lock().is_ok());
    assert!(state.try_lock_agent_lifecycle("restoring").await.is_none());
    assert!(state
        .try_lock_agent_lifecycle("another-agent")
        .await
        .is_some());
    // Cancellation/error unwinding also releases the claim through Drop.
    drop(claim);
    assert!(state.try_lock_agent_lifecycle("restoring").await.is_some());
}

#[tokio::test]
async fn final_persistence_releases_maps_while_a_durable_writer_is_active() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let home = TestHome::new();
    let state = AppState::new();
    let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .unwrap()
        .unwrap();
    let mut persist = std::pin::pin!(persist_roster(&state));
    assert!(poll_fn(|cx| Poll::Ready(persist.as_mut().poll(cx)))
        .await
        .is_pending());
    assert!(state.agents.try_lock().is_ok());
    assert!(state.agent_order.try_lock().is_ok());
    // Publish a newer roster while startup is waiting. Only the current
    // snapshot may be persisted when the competing durable writer releases.
    let config = AgentConfig {
        session_id: "newer".into(),
        session_persistence: AgentSessionPersistenceOverride::Fresh,
        ..Default::default()
    };
    state.agents.lock().await.insert(
        config.session_id.clone(),
        crate::restored_agent_without_process(config.clone(), "Off", String::new(), None, None),
    );
    state.agent_order.lock().await.push(config.session_id);
    drop(barrier);
    persist.await.unwrap();
    let disk: Vec<AgentConfig> = serde_json::from_slice(
        &std::fs::read(home.directory.path().join("settings/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(disk.len(), 1);
    assert_eq!(
        disk[0].session_persistence,
        AgentSessionPersistenceOverride::Fresh
    );
}
