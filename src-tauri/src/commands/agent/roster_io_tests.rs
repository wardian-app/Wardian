use super::super::tests::{make_test_agent, WardianHomeGuard};
use crate::state::AppState;
use tauri::Manager;
use wardian_core::models::AgentConfig;

#[tokio::test(flavor = "current_thread")]
async fn reorder_keeps_executor_and_roster_reads_responsive_during_persistence() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp wardian home");
    unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
    let _home = WardianHomeGuard;
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    for id in ["agent-1", "agent-2"] {
        let agent = make_test_agent();
        agent.config.lock().unwrap().session_id = id.to_string();
        state.agents.lock().await.insert(id.to_string(), agent);
    }
    *state.agent_order.lock().await = vec!["agent-1".into(), "agent-2".into()];
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (heartbeat_tx, heartbeat_rx) = std::sync::mpsc::channel();
    let observer_handle = app.handle().clone();
    // Observe actual persistence entry from outside the executor, then always
    // release it so the unfixed current-thread route can finish and be joined.
    let supervisor = std::thread::spawn(move || {
        let entered = started_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .is_ok();
        let state = observer_handle.state::<AppState>();
        let map_accessible = state.agents.try_lock().is_ok();
        let order_accessible = state.agent_order.try_lock().is_ok();
        let responsive = entered
            && heartbeat_rx
                .recv_timeout(std::time::Duration::from_millis(500))
                .is_ok();
        let _ = release_tx.send(());
        (responsive, map_accessible, order_accessible)
    });
    let handle = app.handle().clone();
    let reorder = tokio::spawn(async move {
        crate::manager::ROSTER_IO_PROBE
            .scope(
                std::cell::RefCell::new(Some(crate::manager::RosterIoProbe {
                    entered: entered_tx,
                    started: started_tx,
                    release: release_rx,
                })),
                crate::commands::agent::reorder_agents(
                    vec!["agent-2".into(), "agent-1".into()],
                    handle.state::<AppState>(),
                    handle.clone(),
                ),
            )
            .await
    });
    entered_rx
        .await
        .expect("reorder enters the actual atomic save section");
    let _ = heartbeat_tx.send(());
    reorder
        .await
        .expect("reorder joins")
        .expect("reorder succeeds");
    let (responsive, map_accessible, order_accessible) =
        supervisor.join().expect("supervisor joins");
    let persisted: Vec<AgentConfig> = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("settings/state.json"))
            .expect("read reordered configuration"),
    )
    .expect("parse saved configuration");
    assert_eq!(
        persisted
            .iter()
            .map(|config| config.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["agent-2", "agent-1"]
    );
    assert!(responsive, "roster persistence blocked the async executor");
    assert!(
        map_accessible,
        "roster persistence held the agent map during I/O"
    );
    assert!(
        order_accessible,
        "roster persistence held the agent order during I/O"
    );
}

fn controlled_gate() -> (
    crate::manager::RosterIoProbe,
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (request_tx, request_rx) = std::sync::mpsc::channel();
    let supervisor = std::thread::spawn(move || {
        let _ = started_rx.recv_timeout(std::time::Duration::from_secs(3));
        let _ = request_rx.recv_timeout(std::time::Duration::from_secs(3));
        let _ = release_tx.send(());
    });
    (
        crate::manager::RosterIoProbe {
            entered: entered_tx,
            started: started_tx,
            release: release_rx,
        },
        entered_rx,
        request_tx,
        supervisor,
    )
}

#[tokio::test(flavor = "current_thread")]
async fn live_roster_save_captures_changes_after_barrier_admission() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp home");
    unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
    let _home = WardianHomeGuard;
    let state = std::sync::Arc::new(AppState::new());
    let agent = make_test_agent();
    agent.config.lock().unwrap().session_id = "agent-1".into();
    let config = agent.config.clone();
    state.agents.lock().await.insert("agent-1".into(), agent);
    state.agent_order.lock().await.push("agent-1".into());
    let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .expect("barrier")
        .expect("held barrier");
    let (attempt_tx, attempt_rx) = tokio::sync::oneshot::channel();
    let save_state = state.clone();
    let save = tokio::spawn(async move {
        crate::manager::roster_io::ROSTER_BARRIER_ATTEMPT
            .scope(
                std::cell::RefCell::new(Some(attempt_tx)),
                crate::manager::roster_io::save_live_state(&save_state, ()),
            )
            .await
    });
    let attempted = tokio::time::timeout(std::time::Duration::from_secs(3), attempt_rx).await;
    // This update belongs to the fixture's already-admitted writer.
    config.lock().unwrap().session_name = "Changed while save waited".into();
    let admitted = vec![config.lock().unwrap().clone()];
    let admitted_save = crate::manager::try_save_state_snapshot_unlocked(&admitted);
    drop(barrier);
    let result = save.await.expect("save joins");
    assert!(
        matches!(attempted, Ok(Ok(()))),
        "save must reach actual contention"
    );
    admitted_save.expect("admitted writer persists");
    result.expect("pending live save succeeds");
    let persisted: Vec<AgentConfig> = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("settings/state.json")).expect("state"),
    )
    .expect("parse state");
    assert_eq!(persisted[0].session_name, "Changed while save waited");
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_roster_save_retains_physical_and_lifecycle_exclusion() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp home");
    unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
    let _home = WardianHomeGuard;
    let state = std::sync::Arc::new(AppState::new());
    let agent = make_test_agent();
    agent.config.lock().unwrap().session_id = "agent-1".into();
    state.agents.lock().await.insert("agent-1".into(), agent);
    state.agent_order.lock().await.push("agent-1".into());
    let life = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let context = life.clone().lock_owned().await;
    let (probe, entered, release, supervisor) = controlled_gate();
    let task_state = state.clone();
    let save = tokio::spawn(async move {
        crate::manager::ROSTER_IO_PROBE
            .scope(std::cell::RefCell::new(Some(probe)), async move {
                super::lock_agent_roster_for_save(&task_state)
                    .await?
                    .save(context)
                    .await
            })
            .await
    });
    let entered_result = tokio::time::timeout(std::time::Duration::from_secs(3), entered).await;
    save.abort();
    let cancelled = matches!(save.await, Err(error) if error.is_cancelled());
    let lifecycle_retained = life.try_lock().is_err();
    let barrier_retained = wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
        .is_ok_and(|barrier| barrier.is_none());
    let _ = release.send(());
    let completed = crate::manager::roster_io::acquire_roster_barrier().await;
    let joined = supervisor.join();
    assert!(joined.is_ok());
    let completed = completed.expect("physical completion");
    drop(completed);
    assert!(matches!(entered_result, Ok(Ok(()))));
    assert!(cancelled && lifecycle_retained && barrier_retained);
    assert!(life.try_lock().is_ok());
    let persisted: Vec<AgentConfig> = serde_json::from_str(
        &std::fs::read_to_string(temp.path().join("settings/state.json")).expect("state"),
    )
    .expect("parse state");
    assert_eq!(persisted[0].session_id, "agent-1");
}

async fn background_fixture(state: &AppState) -> crate::delivery::native_broker::NativeSessionSpec {
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = "agent-1".into();
        config.provider = "codex".into();
        config.is_off = true;
        config.resume_session = None;
        config.fresh_provider_session_id = None;
    }
    let config = agent.config.lock().unwrap().clone();
    state.agents.lock().await.insert("agent-1".into(), agent);
    state.agent_order.lock().await.push("agent-1".into());
    let generation = state
        .interactions
        .start_provider_input_generation(
            "agent-1",
            wardian_core::control::ProviderInputReadiness::Booting,
            None,
        )
        .await
        .generation;
    crate::delivery::native_broker::NativeSessionSpec {
        target_agent_id: "agent-1".into(),
        provider: "codex".into(),
        generation,
        workspace: std::path::PathBuf::new(),
        config,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_background_publication_finishes_memory_after_disk_commit() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp home");
    unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
    let _home = WardianHomeGuard;
    let state = std::sync::Arc::new(AppState::new());
    let spec = background_fixture(&state).await;
    let config = state
        .agents
        .lock()
        .await
        .get("agent-1")
        .expect("agent")
        .config
        .clone();
    let life = state
        .agent_lifecycle_locks
        .lock()
        .await
        .entry("agent-1".into())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let (probe, committed, release, supervisor) = controlled_gate();
    let task_state = state.clone();
    let publish = tokio::spawn(async move {
        crate::manager::codex_shared::BACKGROUND_PUBLICATION_PROBE
            .scope(
                std::cell::RefCell::new(Some(probe)),
                crate::manager::codex_shared::publish_background_identity(
                    &task_state,
                    &spec,
                    "00000000-0000-4000-8000-000000000001",
                    false,
                ),
            )
            .await
    });
    let commit_result = tokio::time::timeout(std::time::Duration::from_secs(3), committed).await;
    publish.abort();
    let cancelled = matches!(publish.await, Err(error) if error.is_cancelled());
    let disk = std::fs::read_to_string(temp.path().join("settings/state.json"))
        .map_err(|error| error.to_string())
        .and_then(|contents| {
            serde_json::from_str::<Vec<AgentConfig>>(&contents).map_err(|error| error.to_string())
        });
    let before = config.lock().unwrap().resume_session.clone();
    let life_retained = life.try_lock().is_err();
    let barrier_retained = wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
        .is_ok_and(|barrier| barrier.is_none());
    let _ = release.send(());
    let completed = crate::manager::roster_io::acquire_roster_barrier().await;
    let joined = supervisor.join();
    let after = config.lock().unwrap().clone();
    assert!(joined.is_ok());
    let completed = completed.expect("publication finishes");
    drop(completed);
    assert!(matches!(commit_result, Ok(Ok(()))) && cancelled && life_retained && barrier_retained);
    let disk = disk.expect("parse state captured before memory publication");
    assert!(
        before.is_none(),
        "fixture must pause before memory publication"
    );
    assert_eq!(
        disk[0].resume_session.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    assert_eq!(after.resume_session, disk[0].resume_session);
    // AgentConfig's manual serializer omits runtime-only fresh-session provenance.
    assert!(disk[0].fresh_provider_session_id.is_none());
    assert_eq!(
        after.fresh_provider_session_id.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    assert!(life.try_lock().is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn roster_write_failures_preserve_best_effort_and_background_error_policies() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp home");
    unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
    let _home = WardianHomeGuard;
    std::fs::create_dir_all(temp.path().join("settings/state.json"))
        .expect("write-failure fixture");
    let state = AppState::new();
    let spec = background_fixture(&state).await;
    crate::manager::roster_io::save_live_state(&state, 7)
        .await
        .expect("best-effort save continues");
    let error = crate::manager::codex_shared::publish_background_identity(
        &state,
        &spec,
        "00000000-0000-4000-8000-000000000001",
        false,
    )
    .await
    .expect_err("background publication reports disk failure");
    assert!(!error.is_empty());
    let agents = state.agents.lock().await;
    assert!(agents["agent-1"]
        .config
        .lock()
        .unwrap()
        .resume_session
        .is_none());
    assert!(state.agent_lifecycle_locks.lock().await["agent-1"]
        .try_lock()
        .is_ok());
    assert!(
        wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
            .expect("released barrier")
            .is_some()
    );
}

#[test]
fn roster_barrier_waiter_does_not_starve_a_single_blocking_worker() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("isolated runtime");
    runtime.block_on(async {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let _home = WardianHomeGuard;
        let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
            .expect("barrier")
            .expect("held barrier");
        let (attempt_tx, attempt_rx) = tokio::sync::oneshot::channel();
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let supervisor = std::thread::spawn(move || {
            let responsive = progress_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_ok();
            drop(barrier);
            responsive
        });
        let waiter = tokio::spawn(crate::manager::roster_io::ROSTER_BARRIER_ATTEMPT.scope(
            std::cell::RefCell::new(Some(attempt_tx)),
            crate::manager::roster_io::acquire_roster_barrier(),
        ));
        let attempt = tokio::time::timeout(std::time::Duration::from_secs(3), attempt_rx).await;
        let progress = tokio::task::spawn_blocking(move || {
            let _ = progress_tx.send(());
        });
        progress.await.expect("blocking progress joins");
        let released = waiter
            .await
            .expect("waiter joins")
            .expect("waiter acquires after release");
        let responsive = supervisor.join().expect("supervisor joins");
        drop(released);
        assert!(matches!(attempt, Ok(Ok(()))));
        assert!(
            responsive,
            "barrier waiter starved the physical writer's blocking worker"
        );
    });
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_pause_admission_preserves_local_configuration() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp home");
    unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
    let _home = WardianHomeGuard;
    let state = std::sync::Arc::new(AppState::new());
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = "agent-1".into();
        config.is_off = false;
    }
    state.agents.lock().await.insert("agent-1".into(), agent);
    state.agent_order.lock().await.push("agent-1".into());
    let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .expect("barrier")
        .expect("held barrier");
    let (attempt_tx, attempt_rx) = tokio::sync::oneshot::channel();
    let task_state = state.clone();
    let pause = tokio::spawn(async move {
        crate::manager::roster_io::ROSTER_BARRIER_ATTEMPT
            .scope(std::cell::RefCell::new(Some(attempt_tx)), async move {
                let mut roster = super::lock_agent_roster_for_best_effort_save(&task_state).await;
                roster
                    .agents
                    .get_mut("agent-1")
                    .expect("agent")
                    .config
                    .lock()
                    .unwrap()
                    .is_off = true;
                roster.save(()).await
            })
            .await
    });
    let attempt = tokio::time::timeout(std::time::Duration::from_secs(3), attempt_rx).await;
    pause.abort();
    let cancelled = matches!(pause.await, Err(error) if error.is_cancelled());
    drop(barrier);
    let config = state.agents.lock().await["agent-1"]
        .config
        .lock()
        .unwrap()
        .clone();
    assert!(matches!(attempt, Ok(Ok(()))) && cancelled);
    assert!(
        !config.is_off,
        "cancellation before admission must precede mutation"
    );
    assert!(!temp.path().join("settings/state.json").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn failed_roster_worker_releases_durable_and_lifecycle_exclusion() {
    let _guard = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp home");
    unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
    let _home = WardianHomeGuard;
    let life = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let context = life.clone().lock_owned().await;
    let barrier = crate::manager::roster_io::acquire_roster_barrier()
        .await
        .expect("barrier");
    let result = crate::manager::roster_io::run_roster_io::<()>(barrier, move || {
        let _context = context;
        panic!("controlled roster worker failure");
    })
    .await;
    assert!(result
        .expect_err("panic becomes a join error")
        .contains("Roster I/O task failed"));
    assert!(life.try_lock().is_ok());
    assert!(
        wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
            .expect("released barrier")
            .is_some()
    );
}
