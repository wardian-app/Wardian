use super::super::{remove_agent, tests::make_test_agent};
use crate::state::{AppState, InteractionState};
use std::cell::RefCell;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tauri::Manager;
use wardian_core::models::AgentConfig;

const WATCHDOG: Duration = Duration::from_secs(5);

struct WardianHomeGuard(Option<std::ffi::OsString>);

impl WardianHomeGuard {
    fn install(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", path);
        Self(previous)
    }
}

impl Drop for WardianHomeGuard {
    fn drop(&mut self) {
        // Preserve the controller's private home even when a regression panics.
        // Removing it would expose the profile fallback to remaining guard drops.
        match self.0.take() {
            Some(previous) => std::env::set_var("WARDIAN_HOME", previous),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

struct DatabaseGateSupervisor {
    release: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for DatabaseGateSupervisor {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

async fn fixture(app: &tauri::AppHandle<tauri::test::MockRuntime>) {
    let state = app.state::<AppState>();
    for id in ["victim", "peer"] {
        let agent = make_test_agent();
        {
            let mut config = agent.config.lock().unwrap();
            config.session_id = id.into();
            config.session_name = id.into();
            config.provider = "mock".into();
            config.is_off = true;
        }
        state.agents.lock().await.insert(id.into(), agent);
    }
    *state.agent_order.lock().await = vec!["victim".into(), "peer".into()];
}

fn persisted(home: &std::path::Path) -> Vec<AgentConfig> {
    serde_json::from_slice(&std::fs::read(home.join("settings/state.json")).unwrap()).unwrap()
}

fn database_probe() -> (
    crate::state::interactions::DeleteIoProbe,
    tokio::sync::oneshot::Receiver<()>,
    mpsc::Sender<()>,
    DatabaseGateSupervisor,
) {
    let (entered, receiver) = tokio::sync::oneshot::channel();
    let (started, _started_rx) = mpsc::channel();
    let (release, requested_rx) = mpsc::channel();
    let (physical_release, release_rx) = mpsc::channel();
    // A regression that blocks the sole executor still releases its actual
    // database gate at the watchdog, so RED is an assertion rather than a hang.
    let thread = std::thread::spawn(move || {
        let _ = requested_rx.recv_timeout(WATCHDOG);
        let _ = physical_release.send(());
    });
    let supervisor = DatabaseGateSupervisor {
        release: release.clone(),
        thread: Some(thread),
    };
    (
        crate::state::interactions::DeleteIoProbe {
            entered,
            started,
            release: release_rx,
        },
        receiver,
        release,
        supervisor,
    )
}

async fn change_peer(app: &tauri::AppHandle<tauri::test::MockRuntime>, name: &str) {
    let state = app.state::<AppState>();
    let agents = state.agents.lock().await;
    agents["peer"].config.lock().unwrap().session_name = name.into();
    *state.agent_order.lock().await = vec!["peer".into(), "victim".into()];
}

fn break_database() {
    wardian_core::db::get_db_conn(|connection| {
        connection.execute("DROP TABLE interactions", [])?;
        Ok(())
    })
    .expect("real SQLite deletion failure fixture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_captures_rename_and_order_after_actual_roster_admission() {
    let _env = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().unwrap();
    let _home = WardianHomeGuard::install(temp.path());
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    fixture(app.handle()).await;
    let barrier = crate::manager::roster_io::acquire_roster_barrier()
        .await
        .unwrap();
    let (waiting, waiting_rx) = tokio::sync::oneshot::channel();
    let (captured, captured_rx) = tokio::sync::oneshot::channel();
    let handle = app.handle().clone();
    let watcher = std::thread::spawn(move || {
        crate::manager::ROSTER_SNAPSHOT_CAPTURE.sync_scope(RefCell::new(Some(captured)), || {
            crate::manager::roster_io::ROSTER_BARRIER_ATTEMPT.sync_scope(
                RefCell::new(Some(waiting)),
                || {
                    crate::manager::spawn::persist_runtime_agent_configs(&handle);
                },
            );
        });
    });
    let phase = tokio::time::timeout(WATCHDOG, async {
        tokio::select! {
            result = waiting_rx => { result.unwrap(); "waiting" },
            result = captured_rx => { assert_eq!(result.unwrap()[0].session_name,"victim"); "captured" },
        }
    }).await;
    change_peer(app.handle(), "renamed before admission").await;
    {
        let state = app.state::<AppState>();
        state.agents.lock().await.remove("victim");
        state.agent_order.lock().await.retain(|id| id != "victim");
    }
    drop(barrier);
    tokio::task::spawn_blocking(move || watcher.join().unwrap())
        .await
        .unwrap();
    phase.expect("actual watcher contention or legacy snapshot must be observed");
    let disk = persisted(temp.path());
    assert_eq!(
        disk.iter()
            .map(|config| config.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["peer"]
    );
    assert_eq!(disk[0].session_name, "renamed before admission");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removal_captures_peer_rename_after_actual_roster_admission() {
    let _env = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().unwrap();
    let _home = WardianHomeGuard::install(temp.path());
    wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    fixture(app.handle()).await;
    let barrier = crate::manager::roster_io::acquire_roster_barrier()
        .await
        .unwrap();
    let (waiting, waiting_rx) = tokio::sync::oneshot::channel();
    let (captured, captured_rx) = tokio::sync::oneshot::channel();
    let handle = app.handle().clone();
    let removal = tokio::spawn(async move {
        crate::manager::ROSTER_SNAPSHOT_CAPTURE
            .scope(
                RefCell::new(Some(captured)),
                crate::manager::roster_io::ROSTER_BARRIER_ATTEMPT.scope(
                    RefCell::new(Some(waiting)),
                    remove_agent(
                        "victim".into(),
                        Some("victim"),
                        handle.state::<AppState>(),
                        handle.clone(),
                        true,
                    ),
                ),
            )
            .await
    });
    let phase = tokio::time::timeout(WATCHDOG, async {
        tokio::select! {
            result=waiting_rx => result.map(|_|()),
            result=captured_rx => result.map(|_|()),
        }
    })
    .await;
    change_peer(app.handle(), "rename while delete waited").await;
    drop(barrier);
    removal.await.unwrap().unwrap();
    phase.unwrap().unwrap();
    let disk = persisted(temp.path());
    assert_eq!(disk.len(), 1);
    assert_eq!(disk[0].session_id, "peer");
    assert_eq!(disk[0].session_name, "rename while delete waited");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn database_failure_retains_barrier_and_compensates_from_current_live_roster() {
    let _env = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().unwrap();
    let _home = WardianHomeGuard::install(temp.path());
    wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    fixture(app.handle()).await;
    let (probe, entered, release, _gate) = database_probe();
    let handle = app.handle().clone();
    let removal = tokio::spawn(async move {
        crate::state::interactions::DELETE_IO_PROBE
            .scope(
                RefCell::new(Some(probe)),
                remove_agent(
                    "victim".into(),
                    Some("victim"),
                    handle.state::<AppState>(),
                    handle.clone(),
                    true,
                ),
            )
            .await
    });
    tokio::time::timeout(WATCHDOG, entered)
        .await
        .unwrap()
        .unwrap();
    let contender = tokio::task::spawn_blocking(|| {
        wardian_core::agent_replacement::acquire_agent_roster_barrier(false).unwrap()
    })
    .await
    .unwrap();
    let exclusion_held = contender.is_none();
    drop(contender);
    change_peer(app.handle(), "live name during DB failure").await;
    {
        let state = app.state::<AppState>();
        let agents = state.agents.lock().await;
        agents["victim"].config.lock().unwrap().description = "current stopped incarnation".into();
    }
    break_database();
    release.send(()).unwrap();
    let error = removal.await.unwrap().unwrap_err();
    assert!(error.contains("Failed to delete agent state"));
    assert!(
        exclusion_held,
        "a contender must not enter between filesystem deletion and compensation"
    );
    let disk = persisted(temp.path());
    assert_eq!(disk.len(), 2);
    assert_eq!(disk[0].session_id, "peer");
    assert_eq!(disk[0].session_name, "live name during DB failure");
    assert_eq!(disk[1].description, "current stopped incarnation");
    assert!(disk[1].is_off);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_physical_roster_write_returns_error_and_compensates_current_live_state() {
    let _env = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().unwrap();
    let _home = WardianHomeGuard::install(temp.path());
    wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    fixture(app.handle()).await;
    let state = app.state::<AppState>();
    let initial = {
        let agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        crate::manager::state_configs_snapshot(&agents, &order)
    };
    crate::manager::try_save_state_snapshot(&initial).unwrap();
    let mailbox = state.interactions.subscribe_agent_mailbox("victim").await;
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (started, _started_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let handle = app.handle().clone();
    let removal = tokio::spawn(async move {
        crate::manager::ROSTER_IO_PROBE
            .scope(
                RefCell::new(Some(crate::manager::RosterIoProbe {
                    entered,
                    started,
                    release: release_rx,
                })),
                remove_agent(
                    "victim".into(),
                    Some("victim"),
                    handle.state::<AppState>(),
                    handle.clone(),
                    true,
                ),
            )
            .await
    });
    tokio::time::timeout(WATCHDOG, entered_rx)
        .await
        .unwrap()
        .unwrap();
    change_peer(app.handle(), "current name after physical write failed").await;
    drop(release); // The actual writer's existing gate returns an I/O error.
    let error = removal.await.unwrap().unwrap_err();
    assert!(
        error.contains("Failed to persist agent deletion"),
        "{error}"
    );
    let disk = persisted(temp.path());
    assert_eq!(disk.len(), 2);
    assert_eq!(
        disk[0].session_name,
        "current name after physical write failed"
    );
    assert!(state.agents.lock().await.contains_key("victim"));
    assert!(!mailbox.borrow().deleted);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_remove_retains_barrier_lifecycle_and_lease_until_publication() {
    let _env = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().unwrap();
    let _home = WardianHomeGuard::install(temp.path());
    wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    fixture(app.handle()).await;
    let state = app.state::<AppState>();
    let life = state.agent_lifecycle_lock_for("victim").await;
    let mut mailbox = state.interactions.subscribe_agent_mailbox("victim").await;
    let (fs_entered, fs_rx) = tokio::sync::oneshot::channel();
    let (fs_started, started_rx) = mpsc::channel();
    let (fs_release, release_rx) = mpsc::channel();
    let (heartbeat, heartbeat_rx) = mpsc::channel();
    let supervisor = std::thread::spawn(move || {
        let started = started_rx.recv_timeout(WATCHDOG).is_ok();
        let responsive = started
            && heartbeat_rx
                .recv_timeout(Duration::from_millis(500))
                .is_ok();
        let _ = fs_release.send(());
        responsive
    });
    let (db_probe, db_entered, db_release, _gate) = database_probe();
    let handle = app.handle().clone();
    let removal = tokio::spawn(async move {
        crate::manager::ROSTER_IO_PROBE
            .scope(
                RefCell::new(Some(crate::manager::RosterIoProbe {
                    entered: fs_entered,
                    started: fs_started,
                    release: release_rx,
                })),
                crate::state::interactions::DELETE_IO_PROBE.scope(
                    RefCell::new(Some(db_probe)),
                    remove_agent(
                        "victim".into(),
                        Some("victim"),
                        handle.state::<AppState>(),
                        handle.clone(),
                        true,
                    ),
                ),
            )
            .await
    });
    tokio::time::timeout(WATCHDOG, fs_rx)
        .await
        .unwrap()
        .unwrap();
    removal.abort();
    let cancelled = removal.await.unwrap_err().is_cancelled();
    let life_held = life.try_lock().is_err();
    let leases = wardian_core::conversation_lease::load_leases();
    let acquisition = leases
        .iter()
        .find(|lease| lease.agent_id == "victim" && lease.owner_kind == "agent_lifecycle")
        .map(|lease| lease.acquisition_id.clone());
    let maps_responsive = state.agents.try_lock().is_ok() && state.agent_order.try_lock().is_ok();
    tokio::spawn(async move {
        let _ = heartbeat.send(());
    })
    .await
    .unwrap();
    tokio::time::timeout(WATCHDOG, db_entered)
        .await
        .unwrap()
        .unwrap();
    let contender = tokio::task::spawn_blocking(|| {
        wardian_core::agent_replacement::acquire_agent_roster_barrier(false).unwrap()
    })
    .await
    .unwrap();
    let held = contender.is_none();
    drop(contender);
    let same_acquisition = wardian_core::conversation_lease::load_leases()
        .iter()
        .any(|lease| Some(&lease.acquisition_id) == acquisition.as_ref());
    db_release.send(()).unwrap();
    tokio::time::timeout(WATCHDOG, async {
        while !mailbox.borrow().deleted {
            mailbox.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let guard = tokio::time::timeout(WATCHDOG, life.lock()).await.unwrap();
    drop(guard);
    tokio::time::timeout(WATCHDOG, async {
        while wardian_core::conversation_lease::load_leases()
            .iter()
            .any(|lease| Some(&lease.acquisition_id) == acquisition.as_ref())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        cancelled
            && held
            && life_held
            && same_acquisition
            && maps_responsive
            && supervisor.join().unwrap()
    );
    assert!(!state.agents.lock().await.contains_key("victim"));
    assert_eq!(
        persisted(temp.path())
            .iter()
            .map(|config| config.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["peer"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_remove_rejects_replacement_arc_or_lease_acquisition_before_compensation() {
    let _env = crate::utils::wardian_test_env_lock_async().await;
    for change in ["config-arc", "lease-acquisition"] {
        let temp = tempfile::tempdir().unwrap();
        let _home = WardianHomeGuard::install(temp.path());
        wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        fixture(app.handle()).await;
        let (probe, entered, release, _gate) = database_probe();
        let handle = app.handle().clone();
        let removal = tokio::spawn(async move {
            crate::state::interactions::DELETE_IO_PROBE
                .scope(
                    RefCell::new(Some(probe)),
                    remove_agent(
                        "victim".into(),
                        Some("victim"),
                        handle.state::<AppState>(),
                        handle.clone(),
                        true,
                    ),
                )
                .await
        });
        tokio::time::timeout(WATCHDOG, entered)
            .await
            .unwrap()
            .unwrap();
        if change == "config-arc" {
            let replacement = make_test_agent();
            {
                let mut config = replacement.config.lock().unwrap();
                config.session_id = "victim".into();
                config.session_name = "Replacement".into();
            }
            app.state::<AppState>()
                .agents
                .lock()
                .await
                .insert("victim".into(), replacement);
        } else {
            let mut leases = wardian_core::conversation_lease::load_leases();
            let lease = leases
                .iter_mut()
                .find(|lease| lease.agent_id == "victim")
                .unwrap();
            lease.acquisition_id = "replacement-acquisition".into();
            wardian_core::conversation_lease::save_leases(&leases).unwrap();
        }
        break_database();
        release.send(()).unwrap();
        let error = removal.await.unwrap().unwrap_err();
        assert!(
            error.contains(if change == "config-arc" {
                "incarnation changed"
            } else {
                "lease was lost"
            }),
            "{error}"
        );
        assert!(
            persisted(temp.path())
                .iter()
                .all(|config| config.session_id != "victim"),
            "no stale/replacement resurrection"
        );
        assert!(app
            .state::<AppState>()
            .agents
            .lock()
            .await
            .contains_key("victim"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_sqlite_delete_retains_mutation_gate_until_cache_invalidation() {
    let _env = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().unwrap();
    wardian_core::db::init_db_at_path(&temp.path().join("state.db")).unwrap();
    let state = Arc::new(InteractionState::default());
    let mut mailbox = state.subscribe_agent_mailbox("victim").await;
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (started, started_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (heartbeat, heartbeat_rx) = mpsc::channel();
    let supervisor = std::thread::spawn(move || {
        let started = started_rx.recv_timeout(WATCHDOG).is_ok();
        let responsive = started
            && heartbeat_rx
                .recv_timeout(Duration::from_millis(500))
                .is_ok();
        let _ = release.send(());
        responsive
    });
    let owner = Arc::clone(&state);
    let deletion = tokio::spawn(async move {
        crate::state::interactions::DELETE_IO_PROBE
            .scope(
                RefCell::new(Some(crate::state::interactions::DeleteIoProbe {
                    entered,
                    started,
                    release: release_rx,
                })),
                owner.delete_agent_durable_state("victim"),
            )
            .await
    });
    tokio::time::timeout(WATCHDOG, entered_rx)
        .await
        .unwrap()
        .unwrap();
    deletion.abort();
    let cancelled = deletion.await.unwrap_err().is_cancelled();
    let contender_state = Arc::clone(&state);
    let contender = tokio::spawn(async move {
        contender_state
            .create_message_durable(
                None,
                vec!["victim".into()],
                wardian_core::control::InteractionBodyRef::Inline {
                    body: "late input".into(),
                },
            )
            .await
    });
    assert!(!mailbox.borrow().deleted);
    tokio::spawn(async move {
        let _ = heartbeat.send(());
    })
    .await
    .unwrap();
    tokio::time::timeout(WATCHDOG, async {
        while !mailbox.borrow().deleted {
            mailbox.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(cancelled && supervisor.join().unwrap());
    assert!(contender
        .await
        .unwrap()
        .unwrap_err()
        .contains("agent has been deleted"));
}
