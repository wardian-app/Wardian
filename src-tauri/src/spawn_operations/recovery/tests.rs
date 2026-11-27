use super::*;
use crate::control::test_support::TestWardianHome;
use serde_json::{json, Value};
use wardian_core::agent_spawn::{self, recovery::Action, AgentSpawnRequest, AgentSpawnState};
use wardian_core::db::{self, AgentUpsert};
use wardian_core::native_transport::{NativeSessionBinding, NativeTransportCapabilities};

const ORIGINAL_MODEL: &str = "fixture-original-model";
const CORRECTED_MODEL: &str = "fixture-corrected-model";

fn preparing_operation(home: &TestWardianHome, name: &str, arm_fence: bool) -> (Request, String) {
    let agent_id = agent_spawn::new_operation_id();
    let operation_id = agent_spawn::new_operation_id();
    let workspace = home.path().to_string_lossy().replace('\\', "/");
    agent_spawn::admit(
        &operation_id,
        &AgentSpawnRequest {
            provider: "codex".into(),
            class: "fixture-class".into(),
            name: Some(name.into()),
            workspace: Some(workspace.clone()),
            model: Some(ORIGINAL_MODEL.into()),
            reasoning_effort: None,
        },
        &agent_id,
        None,
    )
    .unwrap();
    let attempt = agent_spawn::claim_queued(&operation_id)
        .unwrap()
        .unwrap()
        .attempt_id
        .unwrap();
    assert!(agent_spawn::set_prepared_identity(
        &operation_id,
        &attempt,
        "fixture-class",
        &workspace
    )
    .unwrap());
    if arm_fence {
        assert!(agent_spawn::record_launch_context(
            &operation_id,
            &attempt,
            r#"{"source":"fixture"}"#
        )
        .unwrap());
    }
    (
        Request {
            operation_id,
            expected_attempt_id: attempt,
            expected_model: CORRECTED_MODEL.into(),
            action: Action::LaunchIfAbsent,
        },
        agent_id,
    )
}

#[tokio::test]
async fn uncertain_spawn_status_and_wait_expose_the_same_exact_attempt() {
    let home = TestWardianHome::new_async().await;
    let (request, _) = preparing_operation(&home, "observer-attempt", true);
    agent_spawn::recover_after_owner_acquired().unwrap();
    let status = agent_spawn::get(&request.operation_id).unwrap().unwrap();
    let (observed, timed_out, _) =
        crate::spawn_operations::wait_for_operation(&request.operation_id, 5_000)
            .await
            .unwrap();
    assert!(!timed_out);
    assert_eq!(observed, status);
    assert_eq!(status.state, AgentSpawnState::OutcomeUnknown);
    assert_eq!(
        serde_json::to_value(status).unwrap()["attempt_id"],
        request.expected_attempt_id
    );
}

#[tokio::test]
async fn committed_spawn_recovery_replays_without_worker_admission() {
    let home = TestWardianHome::new_async().await;
    let (request, agent_id) = preparing_operation(&home, "receipt-before-slot", true);
    agent_spawn::recover_after_owner_acquired().unwrap();
    let plan = recovery::plan_launch(&request).unwrap();
    let lease = agent::acquire_agent_lifecycle_transition_lease_for_identity(
        &agent_id,
        "receipt-before-slot",
        "codex",
        "",
        "recovery-fixture",
    )
    .unwrap();
    let acceptance = recovery::claim_recovery_launch(&request, &plan, &lease).unwrap();
    let slots = Arc::new(Semaphore::new(1));
    let held = slots.clone().acquire_owned().await.unwrap();
    for _ in 0..2 {
        let replay = replay_or_admit_launch(slots.clone(), request.clone(), async |_, _| {
            panic!("a committed receipt cannot start another worker")
        })
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&replay).unwrap(),
            serde_json::to_vec(&acceptance).unwrap()
        );
    }
    assert_eq!(slots.available_permits(), 0);
    drop(held);
    assert_eq!(
        replay_or_admit_launch(slots.clone(), request.clone(), async |_, _| {
            panic!("an available slot cannot make a committed receipt launch again")
        })
        .await,
        Ok(acceptance.clone())
    );
    for mismatch in [
        Request {
            expected_attempt_id: acceptance.accepted_attempt_id,
            ..request.clone()
        },
        Request {
            expected_model: ORIGINAL_MODEL.into(),
            ..request.clone()
        },
        Request {
            action: Action::FinalizeRegistered,
            ..request.clone()
        },
    ] {
        let error = replay_or_admit_launch(slots.clone(), mismatch, async |_, _| {
            panic!("a changed request cannot reach worker admission")
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "spawn_recovery_receipt_mismatch");
    }
    assert_eq!(slots.available_permits(), 1);
}

#[tokio::test]
async fn spawn_recovery_seals_only_a_proved_startup_pre_effect_cut() {
    let home = TestWardianHome::new_async().await;
    let (pre_effect, agent_id) = preparing_operation(&home, "before-fence", true);
    let (post_effect, _) = preparing_operation(&home, "after-fence", true);
    let (unwitnessed, _) = preparing_operation(&home, "legacy-preparing", false);
    let (unknown, _) = preparing_operation(&home, "already-unknown", true);
    let (changed_owner, _) = preparing_operation(&home, "changed-owner", true);
    assert!(agent_spawn::mark_launching(
        &post_effect.operation_id,
        &post_effect.expected_attempt_id
    )
    .unwrap());
    assert!(
        agent_spawn::mark_outcome_unknown(&unknown.operation_id, &unknown.expected_attempt_id)
            .unwrap()
    );
    let start_plan = agent_spawn::plan(&pre_effect.operation_id)
        .unwrap()
        .unwrap();
    let proof = || {
        db::get_db_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM agent_spawn_codex_pre_effect_witnesses",
                [],
                |row| row.get::<_, u32>(0),
            )?)
        })
        .unwrap()
    };
    db::get_db_conn(|conn| {
        conn.execute(
            "UPDATE agent_spawn_operations SET launch_context_json = ?2 WHERE operation_id = ?1",
            rusqlite::params![unwitnessed.operation_id, r#"{"source":"legacy-fixture"}"#],
        )?;
        conn.execute(
            "UPDATE agent_name_claims SET owner_id = 'foreign' WHERE owner_id = ?1",
            [&changed_owner.operation_id],
        )?;
        conn.execute_batch(
            "CREATE TRIGGER fail_quarantine BEFORE UPDATE OF state ON agent_spawn_operations
             WHEN NEW.state = 'outcome_unknown' BEGIN SELECT RAISE(ABORT, 'fixture'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    assert!(agent_spawn::recover_after_owner_acquired().is_err());
    assert_eq!(proof(), 0);
    assert_eq!(
        agent_spawn::plan(&pre_effect.operation_id).unwrap(),
        Some(start_plan.clone())
    );
    db::get_db_conn(|conn| Ok(conn.execute_batch("DROP TRIGGER fail_quarantine")?)).unwrap();

    assert!(agent_spawn::recover_after_owner_acquired()
        .unwrap()
        .is_empty());
    assert!(agent_spawn::recover_after_owner_acquired()
        .unwrap()
        .is_empty());
    assert_eq!(proof(), 1);
    let state = AppState::new();
    assert_eq!(
        recovery::plan_launch(&pre_effect)
            .unwrap()
            .plan
            .operation
            .agent_id,
        agent_id
    );
    for (request, code) in [
        (&post_effect, "spawn_recovery_launch_absence_unverified"),
        (&unwitnessed, "spawn_recovery_launch_absence_unverified"),
        (&unknown, "spawn_recovery_launch_absence_unverified"),
        (&changed_owner, "spawn_recovery_claim_changed"),
    ] {
        for model in [CORRECTED_MODEL, CORRECTED_MODEL, "fixture-other-model"] {
            let mut request = request.clone();
            request.expected_model = model.into();
            assert_eq!(
                admit_launch(&state, request).await.err().unwrap().code,
                code
            );
        }
    }
    let retained = agent_spawn::plan(&pre_effect.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.operation.state, AgentSpawnState::OutcomeUnknown);
    assert_eq!(retained.operation.agent_id, agent_id);
    assert_eq!(retained.attempt_id, start_plan.attempt_id);
    assert_eq!(retained.request, start_plan.request);
    assert!(agent_spawn::restore_is_excluded(&agent_id).unwrap());
    assert!(!agent_spawn::mark_launching(
        &pre_effect.operation_id,
        &pre_effect.expected_attempt_id
    )
    .unwrap());
    assert!(agent_spawn::claim_queued(&pre_effect.operation_id)
        .unwrap()
        .is_none());
    assert!(state.agents.lock().await.is_empty());

    // A matching old attempt ID alone cannot reuse the witness after its
    // captured preparation changes.
    db::get_db_conn(|conn| {
        conn.execute(
            "UPDATE agent_spawn_operations SET launch_context_json = ?2 WHERE operation_id = ?1",
            rusqlite::params![pre_effect.operation_id, r#"{"source":"changed"}"#],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        admit_launch(&state, pre_effect).await.err().unwrap().code,
        "spawn_recovery_launch_absence_unverified"
    );
    assert_eq!(proof(), 1);
}

#[tokio::test]
async fn spawn_recovery_guarded_launch_materializes_once_and_retargets_same_acquisition() {
    exercise_guarded_registration(None).await;
}

#[tokio::test]
async fn spawn_recovery_expired_after_handoff_cannot_commit_registration() {
    exercise_guarded_registration(Some("expired")).await;
}

#[tokio::test]
async fn spawn_recovery_replaced_after_handoff_signals_loss_and_cannot_commit_registration() {
    exercise_guarded_registration(Some("replaced")).await;
}

#[tokio::test]
async fn spawn_recovery_durable_write_failure_cleans_capture_with_live_lease() {
    exercise_guarded_registration(Some("write-failure")).await;
}

#[tokio::test]
async fn spawn_recovery_reader_eof_after_commit_cleans_runtime_and_keeps_registration() {
    exercise_guarded_registration(Some("reader-eof")).await;
}

#[derive(Debug, Clone)]
struct RecoveryFixtureChild(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl portable_pty::ChildKiller for RecoveryFixtureChild {
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.store(true, Ordering::Release);
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

impl portable_pty::Child for RecoveryFixtureChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(self
            .0
            .load(Ordering::Acquire)
            .then(|| portable_pty::ExitStatus::with_exit_code(0)))
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        portable_pty::Child::try_wait(self)?
            .ok_or_else(|| std::io::Error::other("fixture child is alive"))
    }

    fn process_id(&self) -> Option<u32> {
        None
    }

    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

async fn exercise_guarded_registration(case: Option<&str>) {
    use crate::commands::agent::{
        bootstrap_prepared_agent_spawn, prepare_durable_agent_spawn, RecoveryPendingRegistration,
        RecoveryRegistrationLease, RegisteredAgentSpawn,
    };
    use crate::manager::codex_onboarding::RecoveryRuntimeCleanup;
    use crate::spawn_operations::{finish_prepared_operation, WorkerOutcomeGuard};
    use crate::state::terminal_session::TerminalRuntimeHandles;
    use tauri::Manager;

    let lease_loss = case.filter(|case| matches!(*case, "expired" | "replaced"));
    let write_failure = case == Some("write-failure");
    let reader_eof = case == Some("reader-eof");
    let failed_registration = lease_loss.is_some() || write_failure;
    struct ScopedEnv(&'static str, Option<std::ffi::OsString>);
    impl ScopedEnv {
        fn set(name: &'static str, value: &std::path::Path) -> Self {
            let old = std::env::var_os(name);
            std::env::set_var(name, value);
            Self(name, old)
        }
    }
    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            if let Some(value) = &self.1 {
                std::env::set_var(self.0, value);
            } else {
                std::env::remove_var(self.0);
            }
        }
    }

    let home = TestWardianHome::new_async().await;
    let bin = home.path().join("bin");
    let upstream = home.path().join("upstream");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&upstream).unwrap();
    // Readiness only checks presence. This fixture cannot execute as Codex.
    std::fs::write(
        bin.join(if cfg!(windows) { "codex.exe" } else { "codex" }),
        b"fixture",
    )
    .unwrap();
    let _path = ScopedEnv::set("PATH", &bin);
    let _upstream = ScopedEnv::set("CODEX_HOME", &upstream);
    let create_witness = |name: &str| {
        let agent_id = agent_spawn::new_operation_id();
        let operation_id = agent_spawn::new_operation_id();
        let request = AgentSpawnRequest {
            provider: "codex".into(),
            class: "Generalist".into(),
            name: Some(name.into()),
            workspace: Some(home.path().to_string_lossy().replace('\\', "/")),
            model: Some(ORIGINAL_MODEL.into()),
            reasoning_effort: None,
        };
        agent_spawn::admit(&operation_id, &request, &agent_id, None).unwrap();
        let plan = agent_spawn::claim_queued(&operation_id).unwrap().unwrap();
        let attempt = plan.attempt_id.clone().unwrap();
        let prepared = prepare_durable_agent_spawn(request, plan).unwrap();
        (
            Request {
                operation_id,
                expected_attempt_id: attempt,
                expected_model: CORRECTED_MODEL.into(),
                action: Action::LaunchIfAbsent,
            },
            prepared.config,
        )
    };
    let (request, initial_config) = create_witness("guarded-materialization");
    let (crash_request, _) = create_witness("guarded-crash");
    let (expired_request, expired_config) = create_witness("guarded-expired");
    agent_spawn::recover_after_owner_acquired().unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    state.agent_roster_restored.store(true, Ordering::Release);
    let agent_id = &initial_config.session_id;
    let exact_identity = recovery::plan_launch(&request).unwrap();

    let active = crate::restored_agent_without_process(
        initial_config.clone(),
        "Idle",
        "fixture".into(),
        None,
        None,
    );
    state.agents.lock().await.insert(agent_id.clone(), active);
    assert_eq!(
        admit_launch(&state, request.clone())
            .await
            .err()
            .unwrap()
            .code,
        "spawn_recovery_agent_occupied"
    );
    state.agents.lock().await.remove(agent_id);
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&initial_config)).unwrap();
    assert_eq!(
        admit_launch(&state, request.clone())
            .await
            .err()
            .unwrap()
            .code,
        "spawn_recovery_agent_occupied"
    );
    std::fs::write(home.path().join("settings/state.json"), b"[]").unwrap();
    let existing_lease =
        crate::commands::agent::acquire_agent_lifecycle_transition_lease_for_identity(
            agent_id,
            &initial_config.session_name,
            "codex",
            "",
            "fixture",
        )
        .unwrap();
    assert_eq!(
        admit_launch(&state, request.clone())
            .await
            .err()
            .unwrap()
            .code,
        "spawn_recovery_lease_conflict"
    );
    drop(existing_lease);
    let changed_source = upstream.join("config.toml");
    std::fs::write(&changed_source, b"model = 'fixture-foreign-model'").unwrap();
    assert_eq!(
        admit_launch(&state, request.clone())
            .await
            .err()
            .unwrap()
            .code,
        "spawn_recovery_preparation_changed"
    );
    std::fs::remove_file(changed_source).unwrap();

    let mut first = Box::pin(admit_launch(&state, request.clone()));
    let mut second = Box::pin(admit_launch(&state, request.clone()));
    let (accepted, competing) = tokio::select! {
        result = &mut first => (result, second),
        result = &mut second => (result, first),
    };
    let (receipt, prepared, bootstrap) = accepted.unwrap();
    assert_eq!(&receipt.agent_id, agent_id);
    assert_eq!(receipt.original_attempt_id, request.expected_attempt_id);
    assert_ne!(receipt.accepted_attempt_id, request.expected_attempt_id);
    assert_eq!(receipt.effective_model, CORRECTED_MODEL);
    let owner = bootstrap.lease.owner().clone();
    assert_eq!(
        recovery::claim_recovery_launch(&request, &exact_identity, &bootstrap.lease)
            .unwrap_err()
            .code,
        "spawn_recovery_witness_used"
    );
    let mut different = request.clone();
    different.expected_model = "fixture-different-model".into();
    assert_eq!(
        admit_launch(&state, different).await.err().unwrap().code,
        "spawn_recovery_witness_used"
    );
    assert!(agent_spawn::queued_operation_ids().unwrap().is_empty());
    let keys = (
        receipt.operation_id.clone(),
        receipt.accepted_attempt_id.clone(),
    );
    assert_eq!(
        agent_spawn::get(&keys.0).unwrap().unwrap().state,
        AgentSpawnState::Preparing
    );
    let launch_calls = std::cell::Cell::new(0);
    let launch_state = &state;
    let lease_owner = &owner;
    let calls = &launch_calls;
    let app_handle = app.handle();
    let handoff_probe = std::cell::RefCell::new(None);
    let probe = &handoff_probe;
    let replacement_probe = std::cell::RefCell::new(None);
    let replacement = &replacement_probe;
    let child_stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let child = &child_stopped;
    let provider_generation = 7;
    let (terminal_input, _terminal_input_receiver) = tokio::sync::mpsc::channel(1);
    let runtime_generation = state
        .terminal_sessions
        .start_or_replace_runtime(
            agent_id,
            TerminalRuntimeHandles::new(terminal_input, |_| Ok(())),
            wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
        )
        .await
        .unwrap();
    let worker = finish_prepared_operation(
        &state,
        WorkerOutcomeGuard {
            operation_id: keys.0.clone(),
            attempt_id: keys.1.clone(),
            terminal: false,
        },
        prepared,
        Some(bootstrap),
        |mut prepared, recovery, keys| async move {
            calls.set(calls.get() + 1);
            assert_eq!(
                agent_spawn::get(&keys.0).unwrap().unwrap().state,
                AgentSpawnState::Launching
            );
            assert!(!agent_spawn::mark_launching(&keys.0, &keys.1).unwrap());
            let inherited = bootstrap_prepared_agent_spawn(&mut prepared, Some(&keys), recovery)
                .await
                .unwrap()
                .unwrap();
            let reentered = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                launch_state.lock_agent_lifecycle(agent_id),
            )
            .await
            .expect("registration must be able to reacquire local exclusion");
            drop(reentered);
            assert_eq!(inherited.lease.owner(), lease_owner);
            let native_id = prepared.config.resume_session.as_deref().unwrap();
            assert_eq!(
                prepared.config.fresh_provider_session_id.as_deref(),
                Some(native_id)
            );
            assert_eq!(prepared.config.model.as_deref(), Some(CORRECTED_MODEL));
            assert_eq!(
                serde_json::to_value(&prepared.config).unwrap()["model"],
                CORRECTED_MODEL
            );
            assert_eq!(
                agent_spawn::plan(&keys.0)
                    .unwrap()
                    .unwrap()
                    .provider_session_id
                    .as_deref(),
                Some(native_id)
            );
            let (registration_lease, ()) =
                crate::manager::spawn::prepare_codex_owner_after_reservation(
                    &prepared.config,
                    Some(inherited.lease),
                    Some(&inherited.heartbeat),
                    || async { Ok(()) },
                )
                .await
                .unwrap();
            assert_eq!(registration_lease.owner(), lease_owner);
            assert!(wardian_core::conversation_lease::load_leases_checked()
                .unwrap()
                .iter()
                .any(|lease| &lease.owner() == lease_owner && lease.resume_session == native_id,));

            // Substitute only native completion. The real worker still has
            // to commit this exact incarnation from live and saved evidence.
            crate::manager::try_save_state_snapshot(std::slice::from_ref(&prepared.config))
                .unwrap();
            let saved = persisted_roster().unwrap();
            assert_eq!(saved[0].resume_session.as_deref(), Some(native_id));
            assert!(saved[0].fresh_provider_session_id.is_none());
            db::upsert_agent(&AgentUpsert {
                session_id: agent_id,
                session_name: &prepared.config.session_name,
                description: "",
                agent_class: &prepared.config.agent_class,
                provider: "codex",
                workspace: Some(&prepared.config.folder),
                project: None,
                is_off: false,
                created_at: None,
            })
            .unwrap();
            let mut active = crate::restored_agent_without_process(
                prepared.config.clone(),
                if reader_eof { "Off" } else { "Idle" },
                "fixture".into(),
                None,
                None,
            );
            active.runtime_generation = Some(runtime_generation);
            active.child_process = Some(Box::new(RecoveryFixtureChild(child.clone())));
            active
                .watch_state
                .lock()
                .unwrap()
                .set_codex_attachment_ready(false);
            launch_state
                .native_delivery
                .install_recovery_cleanup_fixture(agent_id, provider_generation)
                .await;
            let publication =
                std::sync::Arc::new(crate::manager::RegistrationPublicationState::for_recovery(
                    lease_owner.clone(),
                    agent_id.clone(),
                    native_id.to_owned(),
                ));
            let (watcher, handoff) =
                crate::manager::spawn::start_recovery_registration_watcher_fixture(
                    RecoveryRegistrationLease {
                        lease: registration_lease,
                        heartbeat: inherited.heartbeat,
                    },
                    &prepared.config,
                    app_handle.clone(),
                    active.current_status.clone(),
                    runtime_generation,
                    publication.clone(),
                );
            handoff.await.unwrap().unwrap();
            assert!(!publication.recovery_lease_lost());
            let cleanup = RecoveryRuntimeCleanup::fixture(
                app_handle.clone(),
                &active,
                provider_generation,
                publication.clone(),
            );
            let reader_alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(!reader_eof));
            let registered = RegisteredAgentSpawn {
                config: prepared.config,
                config_handle: active.config.clone(),
                runtime_generation: active.runtime_generation,
                recovery: Some(
                    RecoveryPendingRegistration::fixture(publication.clone())
                        .with_fixture_capture(cleanup, reader_alive),
                ),
            };
            launch_state
                .agents
                .lock()
                .await
                .insert(agent_id.clone(), active);
            if write_failure {
                assert!(publication.with_recovery_publication(|| ()).is_ok());
                db::get_db_conn(|conn| {
                    conn.execute_batch(
                        "CREATE TRIGGER fixture_reject_registration
                         BEFORE UPDATE OF state ON agent_spawn_operations
                         WHEN NEW.state = 'registered'
                         BEGIN SELECT RAISE(ABORT, 'fixture refused registration'); END;",
                    )?;
                    Ok(())
                })
                .unwrap();
            }
            if let Some(loss) = lease_loss {
                let mut leases = wardian_core::conversation_lease::load_leases_checked().unwrap();
                let entry = leases
                    .iter_mut()
                    .find(|lease| &lease.owner() == lease_owner)
                    .unwrap();
                if loss == "expired" {
                    entry.expires_at =
                        (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
                } else {
                    entry.acquisition_id = agent_spawn::new_operation_id();
                    replacement.replace(Some(entry.clone()));
                }
                wardian_core::conversation_lease::save_leases(&leases).unwrap();
                if loss == "replaced" {
                    tokio::time::pause();
                    tokio::time::advance(std::time::Duration::from_secs(61)).await;
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        publication.wait_for_recovery_lease_loss(),
                    ).await.expect("the readiness watcher must signal lease loss while registration is pending");
                    tokio::time::resume();
                } else {
                    // The next heartbeat has not observed this loss. The
                    // synchronous success boundary must check the live file.
                    assert!(!publication.recovery_lease_lost());
                }
            }
            probe.replace(Some((watcher, publication)));
            Ok(registered)
        },
    );
    let ((), competing_result) = tokio::join!(worker, competing);
    assert_eq!(launch_calls.get(), 1);
    assert_eq!(
        agent_spawn::get(&keys.0).unwrap().unwrap().state,
        if failed_registration {
            AgentSpawnState::OutcomeUnknown
        } else {
            AgentSpawnState::Registered
        }
    );
    assert!(matches!(
        competing_result.err().unwrap().code,
        "spawn_recovery_witness_used" | "spawn_recovery_agent_occupied"
    ));
    assert_eq!(
        agent_spawn::plan(&keys.0).unwrap().unwrap().request,
        exact_identity.plan.request
    );
    let (watcher, publication) = handoff_probe.into_inner().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), watcher)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(publication.recovery_lease_lost(), lease_loss.is_some());
    let leases = wardian_core::conversation_lease::load_leases_checked().unwrap();
    if let Some(replacement) = replacement_probe.into_inner() {
        assert!(leases.contains(&replacement));
    }
    let stopped = failed_registration || reader_eof;
    assert_eq!(child_stopped.load(Ordering::Acquire), stopped);
    assert_eq!(
        state
            .native_delivery
            .codex_owner_selected(agent_id, provider_generation)
            .await,
        !stopped
    );
    assert_eq!(
        state.terminal_sessions.broker_state(agent_id).await.is_ok(),
        !stopped
    );
    if failed_registration {
        assert!(agent_spawn::restore_is_excluded(agent_id).unwrap());
        assert!(agent_spawn::queued_operation_ids().unwrap().is_empty());
        assert_eq!(
            admit_launch(&state, request).await.err().unwrap().code,
            "spawn_recovery_witness_used"
        );
        assert!(!state.agents.lock().await.contains_key(agent_id));
        if write_failure {
            assert!(leases.iter().any(|lease| lease.owner() == owner));
        }
        return;
    }
    if reader_eof {
        let agents = state.agents.lock().await;
        let shell = agents.get(agent_id).expect("durable registered identity");
        assert!(shell.child_process.is_none() && shell.runtime_generation.is_none());
        assert_eq!(*shell.current_status.lock().unwrap(), "Error");
        let config = shell.config.lock().unwrap().clone();
        assert!(persisted_roster().unwrap().iter().any(|saved| {
            saved.session_id == *agent_id
                && saved.resume_session == config.resume_session
                && saved.model.as_deref() == Some(CORRECTED_MODEL)
        }));
        assert!(agent_spawn::queued_operation_ids().unwrap().is_empty());
        assert!(leases.iter().any(|lease| lease.owner() == owner));
        return;
    }
    assert!(state.agents.lock().await[agent_id].child_process.is_some());
    state
        .native_delivery
        .dispose_codex_generation(agent_id, provider_generation)
        .await
        .unwrap();
    assert!(!leases.iter().any(|lease| lease.owner() == owner));
    publication.renew_recovery_lease().unwrap();
    assert!(!publication.recovery_lease_lost());

    let (expired, prepared, bootstrap) =
        admit_launch(&state, expired_request.clone()).await.unwrap();
    let mut leases = wardian_core::conversation_lease::load_leases_checked().unwrap();
    leases
        .iter_mut()
        .find(|entry| entry.owner() == *bootstrap.lease.owner())
        .unwrap()
        .expires_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    wardian_core::conversation_lease::save_leases(&leases).unwrap();
    finish_prepared_operation(
        &state,
        WorkerOutcomeGuard {
            operation_id: expired.operation_id.clone(),
            attempt_id: expired.accepted_attempt_id.clone(),
            terminal: false,
        },
        prepared,
        Some(bootstrap),
        |mut prepared, recovery, keys| async move {
            bootstrap_prepared_agent_spawn(&mut prepared, Some(&keys), recovery).await?;
            panic!("an expired acquisition cannot create a recovery rollout");
        },
    )
    .await;
    assert!(!home
        .path()
        .join("agents")
        .join(expired_config.session_id)
        .exists());
    assert_eq!(
        agent_spawn::get(&expired.operation_id)
            .unwrap()
            .unwrap()
            .state,
        AgentSpawnState::OutcomeUnknown
    );
    assert!(agent_spawn::queued_operation_ids().unwrap().is_empty());
    assert_eq!(
        admit_launch(&state, expired_request)
            .await
            .err()
            .unwrap()
            .code,
        "spawn_recovery_witness_used"
    );

    let (crash, _, mut lease) = admit_launch(&state, crash_request.clone()).await.unwrap();
    lease.lease.retain_on_drop();
    drop(lease);
    agent_spawn::recover_after_owner_acquired().unwrap();
    assert_eq!(
        agent_spawn::get(&crash.operation_id)
            .unwrap()
            .unwrap()
            .state,
        AgentSpawnState::OutcomeUnknown
    );
    assert!(agent_spawn::restore_is_excluded(&crash.agent_id).unwrap());
    assert!(agent_spawn::queued_operation_ids().unwrap().is_empty());
    assert_eq!(
        admit_launch(&state, crash_request)
            .await
            .err()
            .unwrap()
            .code,
        "spawn_recovery_witness_used"
    );
    let audited = db::get_db_conn(|conn| Ok(conn.query_row(
        "SELECT COUNT(*) FROM agent_spawn_codex_launch_audit WHERE operation_id=?1 AND effective_model=?2 AND lease_acquisition_id=?3",
        rusqlite::params![keys.0, CORRECTED_MODEL, owner.acquisition_id],
        |row| row.get::<_, u32>(0),
    )?)).unwrap();
    assert_eq!(audited, 1);
}

struct Fixture {
    request: Request,
    config: AgentConfig,
    native_id: String,
}

impl Fixture {
    fn unknown(home: &TestWardianHome, model: &str, recorded: bool) -> Self {
        let id = agent_spawn::new_operation_id();
        let operation_id = agent_spawn::new_operation_id();
        let native_id = agent_spawn::new_operation_id();
        let folder = home.path().to_string_lossy().replace('\\', "/");
        let mut config = AgentConfig {
            session_id: id.clone(),
            session_name: "recovery-fixture".into(),
            agent_class: "fixture-class".into(),
            provider: "codex".into(),
            folder: folder.clone(),
            model: Some(model.into()),
            resume_session: recorded.then(|| native_id.clone()),
            provider_config: wardian_core::models::ProviderConfig::Codex(Default::default()),
            ..Default::default()
        };
        config.mark_provider_config_nested_for_save();
        agent_spawn::admit(
            &operation_id,
            &AgentSpawnRequest {
                provider: "codex".into(),
                class: config.agent_class.clone(),
                name: Some(config.session_name.clone()),
                workspace: Some(folder.clone()),
                model: Some(model.into()),
                reasoning_effort: None,
            },
            &id,
            Some(&native_id),
        )
        .unwrap();
        let attempt = agent_spawn::claim_queued(&operation_id)
            .unwrap()
            .unwrap()
            .attempt_id
            .unwrap();
        assert!(agent_spawn::set_prepared_identity(
            &operation_id,
            &attempt,
            &config.agent_class,
            &folder
        )
        .unwrap());
        assert!(agent_spawn::record_launch_context(
            &operation_id,
            &attempt,
            r#"{"source":"fixture"}"#
        )
        .unwrap());
        assert!(agent_spawn::mark_launching(&operation_id, &attempt).unwrap());
        if recorded {
            assert!(
                agent_spawn::record_provider_session_id(&operation_id, &attempt, &native_id)
                    .unwrap()
            );
            assert!(agent_spawn::record_bootstrap_receipt(
                &operation_id,
                &attempt,
                r#"{"fixture":"original-attempt"}"#
            )
            .unwrap());
        }
        db::upsert_agent(&AgentUpsert {
            session_id: &id,
            session_name: &config.session_name,
            description: "",
            agent_class: &config.agent_class,
            provider: "codex",
            workspace: Some(&config.folder),
            project: None,
            is_off: false,
            created_at: None,
        })
        .unwrap();
        assert!(agent_spawn::recover_after_owner_acquired()
            .unwrap()
            .is_empty());
        Self {
            request: Request {
                operation_id,
                expected_attempt_id: attempt,
                expected_model: model.into(),
                action: Action::FinalizeRegistered,
            },
            config,
            native_id,
        }
    }

    fn attempt_evidence(&self) -> Value {
        db::get_db_conn(|conn| {
            Ok(conn.query_row(
                "SELECT request_json,payload_hash,attempt_id,planned_agent_id,provider_session_id,
                        launch_context_json,bootstrap_receipt_json
                 FROM agent_spawn_operations WHERE operation_id=?1",
                [&self.request.operation_id],
                |row| {
                    Ok(json!([
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ]))
                },
            )?)
        })
        .unwrap()
    }

    fn owner(&self) -> (String, String) {
        db::get_db_conn(|conn| {
            Ok(conn.query_row(
                "SELECT owner_kind,owner_id FROM agent_name_claims WHERE session_name=?1",
                [&self.config.session_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap()
    }

    fn publish_binding(&self, generation: u64) {
        db::upsert_native_session_binding(&NativeSessionBinding {
            target_agent_id: self.config.session_id.clone(),
            generation,
            provider: "codex".into(),
            transport: "codex_app_server_ws".into(),
            provider_session_id: Some(self.native_id.clone()),
            capabilities: NativeTransportCapabilities::degraded("codex", "codex_app_server_ws"),
            observed_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();
    }

    #[cfg(windows)]
    fn set_owner(&self, id: &str) {
        db::get_db_conn(|conn| {
            conn.execute(
                "UPDATE agent_name_claims SET owner_id=?2 WHERE session_name=?1",
                rusqlite::params![self.config.session_name, id],
            )?;
            Ok(())
        })
        .unwrap();
    }
}

#[tokio::test]
async fn spawn_recovery_legacy_pre_effect_crash_and_replays_leave_the_claim_quarantined() {
    let home = TestWardianHome::new_async().await;
    let fixture = Fixture::unknown(&home, ORIGINAL_MODEL, false);
    let state = AppState::new();
    state.agent_roster_restored.store(true, Ordering::Release);
    let evidence = fixture.attempt_evidence();
    let owner = fixture.owner();
    let mut launch = fixture.request.clone();
    launch.action = Action::LaunchIfAbsent;
    launch.expected_model = CORRECTED_MODEL.into();
    for model in [CORRECTED_MODEL, CORRECTED_MODEL, "fixture-another-model"] {
        launch.expected_model = model.into();
        assert_eq!(
            admit_launch(&state, launch.clone())
                .await
                .err()
                .unwrap()
                .code,
            "spawn_recovery_launch_absence_unverified"
        );
    }
    // An unqualified roster entry likewise supplies no pre-effect proof.
    let candidate = crate::restored_agent_without_process(
        fixture.config.clone(),
        "Error",
        "unqualified fixture observation".into(),
        None,
        None,
    );
    state
        .agents
        .lock()
        .await
        .insert(fixture.config.session_id.clone(), candidate);
    assert_eq!(
        admit_launch(&state, launch.clone())
            .await
            .err()
            .unwrap()
            .code,
        "spawn_recovery_launch_absence_unverified"
    );
    assert_eq!(
        recover(&state, fixture.request.clone())
            .await
            .unwrap_err()
            .code,
        "spawn_recovery_native_unverified"
    );
    launch.expected_attempt_id = agent_spawn::new_operation_id();
    assert_eq!(
        admit_launch(&state, launch).await.err().unwrap().code,
        "spawn_recovery_attempt_changed"
    );
    assert_eq!(fixture.attempt_evidence(), evidence);
    assert_eq!(fixture.owner(), owner);
    assert_eq!(
        agent_spawn::get(&fixture.request.operation_id)
            .unwrap()
            .unwrap()
            .state,
        AgentSpawnState::OutcomeUnknown
    );
    assert!(agent_spawn::restore_is_excluded(&fixture.config.session_id).unwrap());
    assert!(agent_spawn::queued_operation_ids().unwrap().is_empty());
    assert!(persisted_roster().unwrap().is_empty());
}

#[tokio::test]
async fn spawn_recovery_restart_keeps_a_saved_unknown_with_stale_binding_quarantined() {
    let home = TestWardianHome::new_async().await;
    let fixture = Fixture::unknown(&home, CORRECTED_MODEL, true);
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&fixture.config)).unwrap();
    fixture.publish_binding(7);
    db::get_db_conn(|conn| {
        conn.execute(
            "UPDATE agents SET last_status='Idle' WHERE session_id=?1",
            [&fixture.config.session_id],
        )?;
        Ok(())
    })
    .unwrap();
    let claim = fixture.owner();
    let original = fixture.attempt_evidence();
    let saved_path = home.path().join("settings/state.json");
    let saved = std::fs::read(&saved_path).unwrap();

    crate::spawn_operations::recover_after_owner_acquired().unwrap();
    assert!(agent_spawn::restore_is_excluded(&fixture.config.session_id).unwrap());
    assert!(agent_spawn::queued_operation_ids().unwrap().is_empty());
    // Startup skips this row before it can publish a live agent.
    let state = AppState::new();
    state.agent_roster_restored.store(true, Ordering::Release);
    assert_eq!(
        recover(&state, fixture.request.clone())
            .await
            .unwrap_err()
            .code,
        "spawn_recovery_native_unverified"
    );
    assert_eq!(fixture.owner(), claim);
    assert_eq!(fixture.attempt_evidence(), original);
    assert_eq!(std::fs::read(saved_path).unwrap(), saved);
    assert!(state.agents.lock().await.is_empty());
}

#[cfg(windows)]
mod registered {
    use super::*;
    use crate::delivery::codex_shared::test_support::{stop_fixture, WriterCase};
    use crate::state::terminal_session::TerminalRuntimeHandles;
    use portable_pty::{Child, ChildKiller, ExitStatus};
    use std::sync::Arc;
    use wardian_core::models::TerminalGeometry;

    #[derive(Debug, Clone)]
    struct FixtureChild;

    impl ChildKiller for FixtureChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(self.clone())
        }
    }

    impl Child for FixtureChild {
        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            Ok(None)
        }
        fn wait(&mut self) -> std::io::Result<ExitStatus> {
            Ok(ExitStatus::with_exit_code(0))
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    fn persist(config: &AgentConfig) {
        crate::manager::try_save_state_snapshot(std::slice::from_ref(config)).unwrap();
    }

    #[tokio::test]
    async fn spawn_recovery_metadata_requires_the_exact_live_saved_and_durable_registration() {
        let home = TestWardianHome::new_async().await;
        let fixture = Fixture::unknown(&home, CORRECTED_MODEL, true);
        let state = AppState::new();
        state.agent_roster_restored.store(true, Ordering::Release);
        let id = &fixture.config.session_id;
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let generation = state
            .terminal_sessions
            .start_or_replace_runtime(
                id,
                TerminalRuntimeHandles::new(tx, |_| Ok(())),
                TerminalGeometry { rows: 24, cols: 80 },
            )
            .await
            .unwrap();
        let mut agent = crate::restored_agent_without_process(
            fixture.config.clone(),
            "Idle",
            String::new(),
            None,
            None,
        );
        agent.runtime_generation = Some(generation);
        agent.child_process = Some(Box::new(FixtureChild));
        let config_handle = agent.config.clone();
        state.agents.lock().await.insert(id.clone(), agent);
        state.agent_order.lock().await.push(id.clone());

        let before = fixture.attempt_evidence();
        let claim = fixture.owner();
        assert_eq!(
            recover(&state, fixture.request.clone())
                .await
                .unwrap_err()
                .code,
            "spawn_recovery_saved_registration_unverified"
        );
        persist(&fixture.config);
        let saved_path = home.path().join("settings/state.json");
        let saved_before = std::fs::read(&saved_path).unwrap();
        assert_eq!(
            recover(&state, fixture.request.clone())
                .await
                .unwrap_err()
                .code,
            "spawn_recovery_native_unverified",
            "an idle TUI candidate does not prove a bound provider owner"
        );
        let native_generation = 7;
        let mut owner = stop_fixture(
            home.path(),
            id,
            native_generation,
            &fixture.native_id,
            WriterCase::Owned,
        )
        .await;
        Arc::get_mut(&mut owner.owner)
            .unwrap()
            .attachment
            .model_override = Some(CORRECTED_MODEL.into());
        state
            .native_delivery
            .install_stop_test_owner(id, native_generation, Some(generation), owner.owner.clone())
            .await;
        assert_eq!(
            recover(&state, fixture.request.clone())
                .await
                .unwrap_err()
                .code,
            "spawn_recovery_native_unverified"
        );
        fixture.publish_binding(native_generation - 1);
        assert_eq!(
            recover(&state, fixture.request.clone())
                .await
                .unwrap_err()
                .code,
            "spawn_recovery_native_unverified"
        );
        fixture.publish_binding(native_generation);

        config_handle.lock().unwrap().model = Some(ORIGINAL_MODEL.into());
        assert_eq!(
            recover(&state, fixture.request.clone())
                .await
                .unwrap_err()
                .code,
            "spawn_recovery_model_mismatch"
        );
        config_handle.lock().unwrap().model = Some(CORRECTED_MODEL.into());
        let mut wrong = fixture.request.clone();
        wrong.expected_model = ORIGINAL_MODEL.into();
        assert_eq!(
            recover(&state, wrong).await.unwrap_err().code,
            "spawn_recovery_model_mismatch"
        );

        let now = chrono::Utc::now();
        let lease = wardian_core::conversation_lease::ConversationLease {
            agent_id: id.clone(),
            provider: "codex".into(),
            resume_session: fixture.native_id.clone(),
            owner_kind: "agent_lifecycle".into(),
            owner_id: "clear:fixture".into(),
            acquisition_id: agent_spawn::new_operation_id(),
            owner_node_id: None,
            mode: "lifecycle_transition".into(),
            started_at: now.to_rfc3339(),
            heartbeat_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::minutes(1)).to_rfc3339(),
        };
        wardian_core::conversation_lease::save_leases(std::slice::from_ref(&lease)).unwrap();
        assert_eq!(
            recover(&state, fixture.request.clone())
                .await
                .unwrap_err()
                .code,
            "spawn_recovery_lease_conflict"
        );
        assert_eq!(
            wardian_core::conversation_lease::load_leases_checked().unwrap(),
            vec![lease]
        );
        std::fs::write(
            wardian_core::conversation_lease::lease_path().unwrap(),
            b"invalid",
        )
        .unwrap();
        assert_eq!(
            recover(&state, fixture.request.clone())
                .await
                .unwrap_err()
                .code,
            "spawn_recovery_leases_unavailable"
        );
        wardian_core::conversation_lease::save_leases(&[]).unwrap();

        let Admission::NeedsLiveProof(plan) = recovery::admit_recovery(&fixture.request).unwrap()
        else {
            panic!("a still uncertain operation requires live proof");
        };
        let proof = RegistrationEvidence {
            config: fixture.config.clone(),
            terminal_generation: generation,
            provider_generation: native_generation,
        };
        fixture.set_owner("changed-owner");
        assert_eq!(
            recovery::finalize_registered(&fixture.request, &plan, &proof, || true)
                .unwrap_err()
                .code,
            "spawn_recovery_claim_changed"
        );
        assert_eq!(fixture.owner().1, "changed-owner");
        fixture.set_owner(&fixture.request.operation_id);
        assert_eq!(
            recovery::finalize_registered(&fixture.request, &plan, &proof, || false)
                .unwrap_err()
                .code,
            "spawn_recovery_native_unverified"
        );
        assert_eq!(fixture.owner(), claim);
        assert_eq!(fixture.attempt_evidence(), before);

        db::get_db_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER reject_fixture_audit BEFORE INSERT
                ON agent_spawn_recovery_audit BEGIN SELECT RAISE(ABORT, 'fixture audit interruption'); END")?;
            Ok(())
        }).unwrap();
        assert_eq!(
            recover(&state, fixture.request.clone())
                .await
                .unwrap_err()
                .code,
            "spawn_recovery_storage_unavailable"
        );
        assert_eq!(fixture.owner(), claim);
        assert_eq!(fixture.attempt_evidence(), before);
        assert_eq!(
            agent_spawn::get(&fixture.request.operation_id)
                .unwrap()
                .unwrap()
                .state,
            AgentSpawnState::OutcomeUnknown
        );
        db::get_db_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_fixture_audit")?;
            Ok(())
        })
        .unwrap();

        let (left, right) = tokio::join!(
            recover(&state, fixture.request.clone()),
            recover(&state, fixture.request.clone())
        );
        let (success, duplicate) = if left.is_ok() {
            (left, right)
        } else {
            (right, left)
        };
        let success = success.unwrap();
        assert_eq!(success.agent_id, *id);
        assert_eq!(
            success.original_attempt_id,
            fixture.request.expected_attempt_id
        );
        assert_eq!(success.action, Action::FinalizeRegistered);
        assert_eq!(success.operation_id, fixture.request.operation_id);
        assert_eq!(success.confirmed_model, CORRECTED_MODEL);
        let committed_operation = agent_spawn::get(&fixture.request.operation_id)
            .unwrap()
            .unwrap();
        let read_audit = || {
            db::get_db_conn(|conn| {
            let rows: (u32, String, String, String) = conn.query_row(
                "SELECT COUNT(*),confirmed_model,original_error_code,finalized_at FROM agent_spawn_recovery_audit WHERE operation_id=?1",
                [&fixture.request.operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            Ok(rows)
        }).unwrap()
        };
        let audit = read_audit();
        assert_eq!(audit.0, 1);
        assert_eq!(audit.1, CORRECTED_MODEL);
        assert_eq!(audit.2, "interrupted_after_claim");

        // The first pipe reply can disappear after the durable commit. The
        // next process may have no roster, provider, or startup readiness.
        crate::spawn_operations::recover_after_owner_acquired().unwrap();
        assert!(!agent_spawn::restore_is_excluded(id).unwrap());
        let restarted = AppState::new();
        let after_reply_loss = recover(&restarted, fixture.request.clone()).await;
        assert_eq!(
            duplicate,
            Ok(success.clone()),
            "the concurrent loser must replay the same receipt; later retry was {after_reply_loss:?}"
        );
        assert_eq!(after_reply_loss, Ok(success.clone()));
        let original_reply = serde_json::to_vec(&success).unwrap();
        assert_eq!(
            serde_json::to_vec(&duplicate.unwrap()).unwrap(),
            original_reply
        );
        assert_eq!(
            serde_json::to_vec(&after_reply_loss.unwrap()).unwrap(),
            original_reply
        );
        for mismatch in [
            Request {
                action: Action::LaunchIfAbsent,
                ..fixture.request.clone()
            },
            Request {
                expected_attempt_id: agent_spawn::new_operation_id(),
                ..fixture.request.clone()
            },
            Request {
                expected_model: ORIGINAL_MODEL.into(),
                ..fixture.request.clone()
            },
        ] {
            assert_eq!(
                recover(&restarted, mismatch).await.unwrap_err().code,
                "spawn_recovery_receipt_mismatch"
            );
        }
        // A caller can also reach the durable fence with a stale plan.
        // Receipt replay must never consult new live or lease evidence.
        std::fs::write(
            wardian_core::conversation_lease::lease_path().unwrap(),
            b"invalid after commit",
        )
        .unwrap();
        let mut stale = proof;
        stale.config.resume_session = None;
        let replay = recovery::finalize_registered(&fixture.request, &plan, &stale, || {
            panic!("replaying an audit receipt must not check the native owner")
        })
        .unwrap();
        assert_eq!(serde_json::to_vec(&replay).unwrap(), original_reply);
        assert_eq!(fixture.owner(), ("agent".into(), id.clone()));
        assert_eq!(fixture.attempt_evidence(), before);
        assert_eq!(committed_operation.state, AgentSpawnState::Registered);
        assert_eq!(
            agent_spawn::get(&fixture.request.operation_id).unwrap(),
            Some(committed_operation)
        );
        assert_eq!(read_audit(), audit);
        assert_eq!(std::fs::read(saved_path).unwrap(), saved_before);
        assert!(Arc::ptr_eq(
            &state.agents.lock().await[id].config,
            &config_handle
        ));
        assert!(
            owner.requests.lock().unwrap().is_empty(),
            "metadata recovery sends no native RPC"
        );
        owner.cleanup().await;
    }
}
