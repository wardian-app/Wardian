//! Real-store cuts through the existing removal and exclusive-owner entry paths.
use super::*;
use wardian_core::agent_spawn as ledger;

async fn fixture() -> (tauri::App<tauri::test::MockRuntime>, AgentConfig) {
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let agent = super::super::tests::make_test_agent();
    let config = {
        let mut config = agent.config.lock().unwrap();
        config.session_id = uuid::Uuid::new_v4().to_string();
        config.session_name = format!("Bound-{}", config.session_id);
        config.provider = "mock".into();
        config.is_off = true;
        config.clone()
    };
    let attempt = uuid::Uuid::new_v4().to_string();
    ledger::reserve_legacy_name(
        &attempt,
        &config.session_id,
        ledger::LegacyNamePurpose::Spawn,
        None,
        &config.session_name,
    )
    .unwrap();
    ledger::begin_legacy_name_effects(&attempt, &config.session_id).unwrap();
    super::super::persist_agent_config(&config, None).unwrap();
    crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&config)).unwrap();
    ledger::complete_legacy_name(&attempt, &config.session_id, std::slice::from_ref(&config))
        .unwrap();
    let state = app.state::<AppState>();
    state
        .agent_order
        .lock()
        .await
        .push(config.session_id.clone());
    state
        .agents
        .lock()
        .await
        .insert(config.session_id.clone(), agent);
    let home = crate::utils::fs::get_wardian_home().unwrap();
    let profile = home.join("agents").join(&config.session_id);
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("retain.txt"), "owned profile").unwrap();
    (app, config)
}

fn sql(statement: &str) {
    wardian_core::db::get_db_conn(|connection| {
        connection.execute_batch(statement)?;
        Ok(())
    })
    .unwrap();
}

async fn remove(
    app: &tauri::App<tauri::test::MockRuntime>,
    config: &AgentConfig,
) -> Result<(), String> {
    remove_agent(
        config.session_id.clone(),
        Some(&config.session_name),
        app.state::<AppState>(),
        app.handle().clone(),
        true,
    )
    .await
}

fn assert_retained(config: &AgentConfig, phase: &str, json_present: bool) {
    assert_eq!(
        ledger::bound_removal_ticket(&config.session_id)
            .unwrap()
            .unwrap()
            .state,
        phase
    );
    assert!(wardian_core::db::get_all_agents()
        .unwrap()
        .iter()
        .any(|row| row.session_id == config.session_id));
    assert_eq!(
        crate::spawn_operations::persisted_roster()
            .unwrap()
            .iter()
            .any(|saved| saved.session_id == config.session_id),
        json_present
    );
    assert!(ledger::claimed_names()
        .unwrap()
        .contains(&config.session_name));
    let home = crate::utils::fs::get_wardian_home().unwrap();
    assert!(home
        .join("agents")
        .join(&config.session_id)
        .join("retain.txt")
        .exists());
}

#[tokio::test]
async fn bound_removal_delete_abort_then_existing_uuid_retry_and_lost_reply_are_idempotent() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let (app, config) = fixture().await;
    sql("CREATE TRIGGER cut_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(ABORT, 'cut'); END;");
    assert!(remove(&app, &config)
        .await
        .unwrap_err()
        .contains("ownership pending"));
    assert_retained(&config, "closure_proved", false);
    sql("DROP TRIGGER cut_delete");
    let error = remove_agent(
        config.session_id.clone(),
        Some("different name"),
        app.state::<AppState>(),
        app.handle().clone(),
        true,
    )
    .await
    .unwrap_err();
    assert!(error.contains("confirmation name"));
    assert_retained(&config, "closure_proved", false);
    remove(&app, &config).await.unwrap();
    remove(&app, &config).await.unwrap();
    assert_eq!(
        ledger::bound_removal_ticket(&config.session_id)
            .unwrap()
            .unwrap()
            .state,
        "done"
    );
    assert!(wardian_core::db::get_all_agents().unwrap().is_empty());
    assert!(crate::spawn_operations::persisted_roster()
        .unwrap()
        .is_empty());
    assert!(app.state::<AppState>().agents.lock().await.is_empty());
}

#[tokio::test]
async fn bound_removal_restart_completes_sealed_metadata_without_roster_or_provider_replay() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (app, config) = fixture().await;
    sql("CREATE TRIGGER cut_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(ABORT, 'cut'); END;");
    assert!(remove(&app, &config).await.is_err());
    drop(app);
    sql("DROP TRIGGER cut_delete");
    wardian_core::db::init_db_at_path(&home.path().join("state.db")).unwrap();
    crate::spawn_operations::recover_after_owner_acquired().unwrap();
    assert_eq!(
        ledger::bound_removal_ticket(&config.session_id)
            .unwrap()
            .unwrap()
            .state,
        "done"
    );
    assert!(wardian_core::db::get_all_agents().unwrap().is_empty());
    assert!(crate::spawn_operations::persisted_roster()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn bound_removal_intent_join_and_seal_cuts_do_not_authorize_restart_deletion() {
    for cut in ["intent", "join", "seal"] {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let (app, config) = fixture().await;
        match cut {
            "intent" => sql("CREATE TRIGGER cut_intent BEFORE UPDATE OF removal_state ON legacy_name_attempts WHEN NEW.removal_state = 'intent' BEGIN SELECT RAISE(ABORT, 'cut'); END;"),
            "join" => {
                // An owner generation without its actual child handle is
                // uncertainty, even though the saved config says Off.
                app.state::<AppState>().agents.lock().await
                    .get_mut(&config.session_id).unwrap().runtime_generation = Some(7);
            }
            "seal" => sql("CREATE TRIGGER cut_seal BEFORE UPDATE OF removal_state ON legacy_name_attempts WHEN NEW.removal_state = 'closure_proved' BEGIN SELECT RAISE(ABORT, 'cut'); END;"),
            _ => unreachable!(),
        }
        let result = if cut == "join" {
            remove_agent(
                config.session_id.clone(),
                Some(&config.session_name),
                app.state::<AppState>(),
                app.handle().clone(),
                false,
            )
            .await
        } else {
            remove(&app, &config).await
        };
        assert!(result.is_err(), "{cut}");
        sql("DROP TRIGGER IF EXISTS cut_intent; DROP TRIGGER IF EXISTS cut_seal;");
        if cut == "intent" {
            assert!(ledger::bound_removal_ticket(&config.session_id)
                .unwrap()
                .is_none());
            assert!(app
                .state::<AppState>()
                .agents
                .lock()
                .await
                .contains_key(&config.session_id));
        } else {
            assert_retained(&config, "intent", false);
            assert!(remove(&app, &config)
                .await
                .unwrap_err()
                .contains("unproved"));
        }
        crate::spawn_operations::recover_after_owner_acquired().unwrap();
        assert!(wardian_core::db::get_all_agents()
            .unwrap()
            .iter()
            .any(|row| row.session_id == config.session_id));
        assert_eq!(
            crate::spawn_operations::persisted_roster().unwrap().len(),
            usize::from(cut == "intent")
        );
    }
}

struct PhysicalGate {
    entered: tokio::sync::oneshot::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for PhysicalGate {
    fn drop(&mut self) {
        let _ = self.release.send(());
        self.worker.take().unwrap().join().unwrap();
    }
}

fn physical_gate() -> (crate::state::interactions::DeleteIoProbe, PhysicalGate) {
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (started, _) = std::sync::mpsc::channel();
    let (requested, requested_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = requested_rx.recv_timeout(Duration::from_secs(5));
        let _ = release.send(());
    });
    (
        crate::state::interactions::DeleteIoProbe {
            entered,
            started,
            release: release_rx,
        },
        PhysicalGate {
            entered: entered_rx,
            release: requested,
            worker: Some(worker),
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_removal_sql_callback_rechecks_replacement_and_new_lease_after_physical_wait() {
    for change in ["replacement", "lease"] {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let (app, config) = fixture().await;
        let (probe, mut gate) = physical_gate();
        let handle = app.handle().clone();
        let target = config.clone();
        let removal = tokio::spawn(async move {
            crate::state::interactions::DELETE_IO_PROBE
                .scope(
                    std::cell::RefCell::new(Some(probe)),
                    remove_agent(
                        target.session_id,
                        Some(&target.session_name),
                        handle.state::<AppState>(),
                        handle.clone(),
                        true,
                    ),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), &mut gate.entered)
            .await
            .unwrap()
            .unwrap();
        if change == "replacement" {
            let replacement = super::super::tests::make_test_agent();
            *replacement.config.lock().unwrap() = config.clone();
            app.state::<AppState>()
                .agents
                .lock()
                .await
                .insert(config.session_id.clone(), replacement);
        } else {
            let mut leases = wardian_core::conversation_lease::load_leases();
            let lease = leases
                .iter_mut()
                .find(|lease| lease.agent_id == config.session_id)
                .unwrap();
            lease.acquisition_id = "replacement-acquisition".into();
            wardian_core::conversation_lease::save_leases(&leases).unwrap();
        }
        gate.release.send(()).unwrap();
        let error = removal.await.unwrap().unwrap_err();
        assert!(
            error.contains(if change == "replacement" {
                "incarnation changed"
            } else {
                "lease was lost"
            }),
            "{error}"
        );
        assert_retained(&config, "closure_proved", false);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_removal_failed_physical_json_write_keeps_intent_without_closure() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (app, config) = fixture().await;
    let (probe, mut gate) = physical_gate();
    let probe = crate::manager::RosterIoProbe {
        entered: probe.entered,
        started: probe.started,
        release: probe.release,
    };
    let handle = app.handle().clone();
    let target = config.clone();
    let removal = tokio::spawn(async move {
        crate::manager::ROSTER_IO_PROBE
            .scope(
                std::cell::RefCell::new(Some(probe)),
                remove_agent(
                    target.session_id,
                    Some(&target.session_name),
                    handle.state::<AppState>(),
                    handle.clone(),
                    true,
                ),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), &mut gate.entered)
        .await
        .unwrap()
        .unwrap();
    let roster = home.path().join("settings/state.json");
    let backup = home.path().join("settings/captured-state.json");
    std::fs::rename(&roster, &backup).unwrap();
    std::fs::create_dir(&roster).unwrap();
    gate.release.send(()).unwrap();
    let error = removal.await.unwrap().unwrap_err();
    std::fs::remove_dir(&roster).unwrap();
    std::fs::rename(&backup, &roster).unwrap();
    assert!(error.contains("removal intent retained"), "{error}");
    assert_retained(&config, "intent", true);
    crate::spawn_operations::recover_after_owner_acquired().unwrap();
    assert_retained(&config, "intent", true);
    assert!(app
        .state::<AppState>()
        .agents
        .lock()
        .await
        .contains_key(&config.session_id));
}

#[tokio::test]
async fn bound_removal_restart_rejects_republished_json_or_replaced_profile() {
    for changed in ["json", "profile"] {
        let home = crate::control::test_support::TestWardianHome::new_async().await;
        let (app, config) = fixture().await;
        sql("CREATE TRIGGER cut_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(ABORT, 'cut'); END;");
        assert!(remove(&app, &config).await.is_err());
        sql("DROP TRIGGER cut_delete");
        if changed == "json" {
            crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&config))
                .unwrap();
        } else {
            let profile = home.path().join("agents").join(&config.session_id);
            std::fs::rename(&profile, profile.with_extension("original")).unwrap();
            std::fs::create_dir(&profile).unwrap();
            std::fs::write(profile.join("retain.txt"), "replacement profile").unwrap();
        }
        crate::spawn_operations::recover_after_owner_acquired().unwrap();
        assert_retained(&config, "closure_proved", changed == "json");
        assert!(remove(&app, &config)
            .await
            .unwrap_err()
            .contains("ownership pending"));
        assert_retained(&config, "closure_proved", changed == "json");
    }
}
