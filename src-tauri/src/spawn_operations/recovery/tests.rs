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
    let (one, two) = tokio::join!(
        recover(&state, pre_effect.clone()),
        recover(&state, pre_effect.clone()),
    );
    assert_eq!(one, two);
    assert_eq!(
        one.unwrap_err().code,
        "spawn_recovery_launch_boundary_unavailable"
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
            assert_eq!(recover(&state, request).await.unwrap_err().code, code);
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
        recover(&state, pre_effect).await.unwrap_err().code,
        "spawn_recovery_launch_absence_unverified"
    );
    assert_eq!(proof(), 1);
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
            recover(&state, launch.clone()).await.unwrap_err().code,
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
        recover(&state, launch.clone()).await.unwrap_err().code,
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
        recover(&state, launch).await.unwrap_err().code,
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
