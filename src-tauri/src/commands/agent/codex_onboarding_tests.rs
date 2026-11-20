use crate::control::test_support::TestWardianHome;
use crate::manager::codex_onboarding::{
    cancellation_requires_rollback, exact_runtime_generation_matches, format_cleanup_errors,
    should_publish_provisionally,
};
use crate::state::{ActiveAgent, AgentWatchState, AppState};
use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::Manager;
use wardian_core::models::{AgentConfig, ProviderConfig};

fn roster_without_session(configs: &[AgentConfig], session_id: &str) -> Vec<AgentConfig> {
    configs
        .iter()
        .filter(|config| config.session_id != session_id)
        .cloned()
        .collect()
}

#[tokio::test]
async fn unrelated_publication_keeps_actual_quarantine_config_after_startup_recovery() {
    let home = TestWardianHome::new_async().await;
    let id = uuid::Uuid::new_v4().to_string();
    let attempt = uuid::Uuid::new_v4().to_string();
    let quarantine = test_active_agent(&id).config.lock().unwrap().clone();
    wardian_core::agent_spawn::reserve_legacy_name(
        &attempt,
        &id,
        wardian_core::agent_spawn::LegacyNamePurpose::Spawn,
        None,
        &quarantine.session_name,
    )
    .unwrap();
    wardian_core::agent_spawn::begin_legacy_name_effects(&attempt, &id).unwrap();
    crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&quarantine)).unwrap();
    crate::spawn_operations::recover_after_owner_acquired().unwrap();
    assert!(wardian_core::agent_spawn::restore_is_excluded(&id).unwrap());
    let state = AppState::new();
    assert_eq!(state.spawn_operation_slots.available_permits(), 0);
    assert!(state.agents.lock().await.is_empty());
    let operation_id = uuid::Uuid::new_v4().to_string();
    let registered_id = uuid::Uuid::new_v4().to_string();
    let request = wardian_core::agent_spawn::AgentSpawnRequest {
        provider: "codex".into(),
        class: "Coder".into(),
        name: Some("unrelated".into()),
        workspace: Some("workspace".into()),
        model: None,
        reasoning_effort: None,
    };
    wardian_core::agent_spawn::admit(&operation_id, &request, &registered_id, None).unwrap();
    assert_eq!(
        wardian_core::agent_spawn::queued_operation_ids().unwrap(),
        vec![operation_id]
    );
    let active = test_active_agent(&registered_id);
    let config = active.config.lock().unwrap().clone();
    state.agents.lock().await.insert(registered_id, active);
    let barrier = crate::manager::roster_io::acquire_roster_barrier()
        .await
        .unwrap();
    crate::manager::roster_io::write_snapshot_strict(
        &barrier,
        home.path().to_path_buf(),
        vec![config],
    )
    .await
    .unwrap();
    let saved = crate::spawn_operations::persisted_roster().unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(
        serde_json::to_value(saved.iter().find(|saved| saved.session_id == id).unwrap()).unwrap(),
        serde_json::to_value(quarantine).unwrap()
    );
    assert!(!state.agents.lock().await.contains_key(&id));
    let state_path = home.path().join("settings/state.json");
    std::fs::write(&state_path, b"malformed").unwrap();
    assert!(crate::manager::try_save_state_snapshot_unlocked(&[]).is_err());
    assert_eq!(std::fs::read(&state_path).unwrap(), b"malformed");
}

#[test]
fn cancelled_registration_marks_publication_failed_without_overriding_commit() {
    let pending = Arc::new(crate::manager::RegistrationPublicationState::default());
    drop(super::RegistrationPublicationAttempt(pending.clone()));
    assert!(pending.is_failed());

    let committed = Arc::new(crate::manager::RegistrationPublicationState::default());
    committed.commit();
    drop(super::RegistrationPublicationAttempt(committed.clone()));
    assert!(!committed.is_failed());
}

#[test]
fn operation_completion_uses_the_original_registration_incarnation() {
    let mut original = test_active_agent("planned-agent");
    original.runtime_generation = Some(7);
    let registered = super::RegisteredAgentSpawn {
        config: original.config.lock().unwrap().clone(),
        config_handle: original.config.clone(),
        runtime_generation: original.runtime_generation,
    };
    assert!(registered.matches(&original));
    let mut replacement = test_active_agent("planned-agent");
    replacement.runtime_generation = Some(7);
    assert!(!registered.matches(&replacement));
    original.runtime_generation = Some(8);
    assert!(!registered.matches(&original));
    original.runtime_generation = Some(7);
    original.config.lock().unwrap().resume_session = Some("different-session".into());
    assert!(!registered.matches(&original));
}

#[tokio::test]
async fn published_spawn_and_clone_keep_incarnation_when_name_promotion_fails() {
    let _home = TestWardianHome::new_async().await;
    for purpose in [
        wardian_core::agent_spawn::LegacyNamePurpose::Spawn,
        wardian_core::agent_spawn::LegacyNamePurpose::Clone,
    ] {
        let state = AppState::new();
        let id = uuid::Uuid::new_v4().to_string();
        let attempt = uuid::Uuid::new_v4().to_string();
        let mut active = test_active_agent(&id);
        let child_exited = Arc::new(AtomicBool::new(false));
        active.child_process = Some(Box::new(FakeChild {
            exited: child_exited.clone(),
        }));
        let expected = active.config.clone();
        let config = expected.lock().unwrap().clone();
        wardian_core::agent_spawn::reserve_legacy_name(&attempt, &id, purpose, None, &id).unwrap();
        wardian_core::agent_spawn::begin_legacy_name_effects(&attempt, &id).unwrap();
        super::super::persist_agent_config(&config, None).unwrap();
        state.agents.lock().await.insert(id.clone(), active);
        state.agent_order.lock().await.push(id.clone());
        crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&config)).unwrap();
        wardian_core::db::get_db_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER cut_legacy_promotion BEFORE UPDATE ON agent_name_claims
                BEGIN SELECT RAISE(ABORT, 'promotion failure'); END;",
            )?;
            Ok(())
        })
        .unwrap();
        let publication = Arc::new(crate::manager::RegistrationPublicationState::default());
        let disposition = super::RegistrationPublicationAttempt(publication.clone());
        publication.commit();
        let error = super::finish_name_ownership(&state, &config, &attempt, &expected, None)
            .await
            .unwrap_err();
        let error = super::ownership_pending(&id, &attempt, &error);
        drop(disposition);
        assert!(super::is_ownership_pending(&error));
        assert!(error.contains(&id) && error.contains(&attempt));
        assert!(!publication.is_failed());
        assert!(!child_exited.load(Ordering::SeqCst));
        assert!(Arc::ptr_eq(
            &state.agents.lock().await[&id].config,
            &expected
        ));
        let roster = crate::spawn_operations::persisted_roster().unwrap();
        assert_eq!(roster[0].session_id, id);
        assert!(wardian_core::agent_spawn::restore_is_excluded(&id).unwrap());
        wardian_core::db::get_db_conn(|conn| {
            conn.execute_batch("DROP TRIGGER cut_legacy_promotion")?;
            Ok(())
        })
        .unwrap();
        wardian_core::agent_spawn::recover_legacy_names(&roster).unwrap();
        assert!(!wardian_core::agent_spawn::restore_is_excluded(&id).unwrap());
        assert!(!child_exited.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn ownership_completion_rejects_a_replaced_live_incarnation() {
    let _home = TestWardianHome::new_async().await;
    let state = AppState::new();
    let id = uuid::Uuid::new_v4().to_string();
    let attempt = uuid::Uuid::new_v4().to_string();
    let original = test_active_agent(&id);
    let expected = original.config.clone();
    let config = expected.lock().unwrap().clone();
    wardian_core::agent_spawn::reserve_legacy_name(
        &attempt,
        &id,
        wardian_core::agent_spawn::LegacyNamePurpose::Spawn,
        None,
        &id,
    )
    .unwrap();
    wardian_core::agent_spawn::begin_legacy_name_effects(&attempt, &id).unwrap();
    super::super::persist_agent_config(&config, None).unwrap();
    crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&config)).unwrap();
    state
        .agents
        .lock()
        .await
        .insert(id.clone(), test_active_agent(&id));
    let error = super::finish_name_ownership(&state, &config, &attempt, &expected, None)
        .await
        .unwrap_err();
    assert!(error.contains("incarnation changed"));
    wardian_core::db::get_db_conn(|conn| {
        let phase: String = conn.query_row(
            "SELECT phase FROM legacy_name_attempts WHERE attempt_id = ?1",
            rusqlite::params![attempt],
            |row| row.get(0),
        )?;
        assert_eq!(phase, "effects_possible");
        Ok(())
    })
    .unwrap();
    assert!(wardian_core::agent_spawn::restore_is_excluded(&id).unwrap());
}

#[tokio::test]
async fn rename_json_failure_and_failed_sql_rollback_retain_bound_disposition() {
    let _home = TestWardianHome::new_async().await;
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let id = uuid::Uuid::new_v4().to_string();
    let active = test_active_agent(&id);
    active.config.lock().unwrap().session_name = "before".into();
    let config = active.config.lock().unwrap().clone();
    super::super::persist_agent_config(&config, None).unwrap();
    state.agents.lock().await.insert(id.clone(), active);
    state.agent_order.lock().await.push(id.clone());
    crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&config)).unwrap();
    wardian_core::db::get_db_conn(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER cut_rename_rollback BEFORE UPDATE ON agents
            WHEN OLD.session_name = 'after' AND NEW.session_name = 'before'
            BEGIN SELECT RAISE(ABORT, 'rollback failure'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    let (entered, _entered_rx) = tokio::sync::oneshot::channel();
    let (started, _started_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    drop(release); // Existing roster seam fails before touching the old JSON.
    let probe = crate::manager::RosterIoProbe {
        entered,
        started,
        release: release_rx,
    };
    let error = crate::manager::ROSTER_IO_PROBE
        .scope(
            std::cell::RefCell::new(Some(probe)),
            super::super::rename_agent(id.clone(), "after".into(), state, app.handle().clone()),
        )
        .await
        .unwrap_err();
    assert!(super::is_ownership_pending(&error));
    assert!(error.contains(&id));
    assert_eq!(
        crate::spawn_operations::persisted_roster().unwrap()[0].session_name,
        "before"
    );
    assert_eq!(
        app.state::<AppState>().agents.lock().await[&id]
            .config
            .lock()
            .unwrap()
            .session_name,
        "before"
    );
    wardian_core::db::get_db_conn(|conn| {
        let row: (String, String) = conn.query_row(
            "SELECT a.session_name, n.phase FROM agents a JOIN legacy_name_attempts n
             ON a.session_id = n.agent_id WHERE a.session_id = ?1",
            rusqlite::params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(row, ("after".into(), "effects_possible".into()));
        let owner: String = conn.query_row(
            "SELECT owner_id FROM agent_name_claims WHERE session_name = 'after'",
            [],
            |row| row.get(0),
        )?;
        assert!(error.contains(&owner));
        Ok(())
    })
    .unwrap();
    wardian_core::agent_spawn::recover_legacy_names(
        &crate::spawn_operations::persisted_roster().unwrap(),
    )
    .unwrap();
    assert!(wardian_core::agent_spawn::restore_is_excluded(&id).unwrap());
}

#[derive(Debug, Clone)]
struct FakeChild {
    exited: Arc<AtomicBool>,
}

impl portable_pty::ChildKiller for FakeChild {
    fn kill(&mut self) -> std::io::Result<()> {
        self.exited.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

impl portable_pty::Child for FakeChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(self
            .exited
            .load(Ordering::SeqCst)
            .then(|| portable_pty::ExitStatus::with_exit_code(0)))
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        panic!("onboarding cleanup must poll the captured child")
    }

    fn process_id(&self) -> Option<u32> {
        None
    }

    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

fn test_active_agent(session_id: &str) -> ActiveAgent {
    ActiveAgent {
        config: Arc::new(Mutex::new(AgentConfig {
            session_id: session_id.into(),
            session_name: session_id.into(),
            provider: "codex".into(),
            provider_config: ProviderConfig::Codex(Default::default()),
            ..Default::default()
        })),
        child_process: None,
        background_processes: Vec::new(),
        memory_capability: None,
        runtime_generation: None,
        process_id: None,
        query_count: Arc::new(Mutex::new(0)),
        init_timestamp: Arc::new(Mutex::new(None)),
        last_query_timestamp: Arc::new(Mutex::new(None)),
        current_status: Arc::new(Mutex::new("Booting".into())),
        last_status_at: Arc::new(Mutex::new(None)),
        watch_state: Arc::new(Mutex::new(AgentWatchState::new(
            session_id.to_string(),
            4096,
            262_144,
        ))),
        terminal_title: Arc::new(Mutex::new(String::new())),
        last_output_at: Arc::new(Mutex::new(None)),
        log_path: Arc::new(Mutex::new(None)),
        log_last_modified: Arc::new(Mutex::new(None)),
        #[cfg(windows)]
        job_object: None,
    }
}

fn detachable_agent(
    session_id: &str,
    provider: &str,
    runtime_generation: Option<u64>,
) -> ActiveAgent {
    let config = AgentConfig {
        session_id: session_id.to_string(),
        session_name: session_id.to_string(),
        provider: provider.to_string(),
        ..AgentConfig::default()
    };
    ActiveAgent {
        config: Arc::new(Mutex::new(config)),
        child_process: Some(Box::new(FakeChild {
            exited: Arc::new(AtomicBool::new(true)),
        })),
        background_processes: Vec::new(),
        memory_capability: None,
        runtime_generation,
        process_id: None,
        query_count: Arc::new(Mutex::new(0)),
        init_timestamp: Arc::new(Mutex::new(None)),
        last_query_timestamp: Arc::new(Mutex::new(None)),
        current_status: Arc::new(Mutex::new("Booting".into())),
        last_status_at: Arc::new(Mutex::new(None)),
        watch_state: Arc::new(Mutex::new(AgentWatchState::new(
            session_id.to_string(),
            4096,
            262_144,
        ))),
        terminal_title: Arc::new(Mutex::new(String::new())),
        last_output_at: Arc::new(Mutex::new(None)),
        log_path: Arc::new(Mutex::new(None)),
        log_last_modified: Arc::new(Mutex::new(None)),
        #[cfg(windows)]
        job_object: None,
    }
}

async fn seeded_detach_state() -> (TestWardianHome, AppState) {
    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    {
        let mut agents = state.agents.lock().await;
        agents.insert(
            "codex-provisional".into(),
            detachable_agent("codex-provisional", "codex", Some(7)),
        );
        agents.insert(
            "claude-live".into(),
            detachable_agent("claude-live", "claude", None),
        );
    }
    state
        .agent_order
        .lock()
        .await
        .extend(["codex-provisional".to_string(), "claude-live".to_string()]);
    (home, state)
}

#[test]
fn fresh_codex_is_visible_provisionally_while_other_providers_keep_sync_path() {
    assert!(should_publish_provisionally("codex", false));
    assert!(!should_publish_provisionally("codex", true));
    assert!(!should_publish_provisionally("claude", false));
    assert!(!should_publish_provisionally("opencode", false));
}

#[test]
fn cancellation_rolls_back_only_after_roster_publication() {
    assert!(!cancellation_requires_rollback(false));
    assert!(cancellation_requires_rollback(true));
}

#[test]
fn stale_finalizer_cannot_match_a_replacement_generation() {
    assert!(exact_runtime_generation_matches(Some(7), 7));
    assert!(!exact_runtime_generation_matches(Some(6), 7));
    assert!(!exact_runtime_generation_matches(None, 7));
}

#[test]
fn rollback_removes_only_the_current_roster_entry() {
    let configs = vec![
        AgentConfig {
            session_id: "codex-provisional".into(),
            provider: "codex".into(),
            ..Default::default()
        },
        AgentConfig {
            session_id: "claude-live".into(),
            provider: "claude".into(),
            ..Default::default()
        },
    ];
    let remaining = roster_without_session(&configs, "codex-provisional");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].session_id, "claude-live");
    assert_eq!(remaining[0].provider, "claude");
}

#[test]
fn failed_finalizer_reports_snapshot_and_stop_fence_failures() {
    let error = format_cleanup_errors(
        "Codex attachment failed",
        Some("state snapshot unavailable".into()),
        None,
        Some("child still running".into()),
    );
    assert!(error.contains("state snapshot unavailable"));
    assert!(error.contains("Codex cleanup retained: child still running"));
}

#[tokio::test]
async fn provisional_install_rejects_an_undurable_roster() {
    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    let config = AgentConfig {
        session_id: "codex-provisional".into(),
        session_name: "codex-provisional".into(),
        provider: "codex".into(),
        provider_config: ProviderConfig::Codex(Default::default()),
        ..Default::default()
    };
    let pending = super::PendingRuntime::prepare(&config, &state.terminal_sessions)
        .expect("reserve provisional stop registration")
        .attach(test_active_agent(&config.session_id));

    let blocked_home = home.path().join("blocked-home");
    std::fs::write(&blocked_home, "not a directory").expect("blocking home");
    unsafe { std::env::set_var("WARDIAN_HOME", &blocked_home) };

    let mut completion = None;
    let (pending, error) = super::commit_registered_agent(
        &state,
        &config.session_id,
        pending,
        &mut completion,
        super::super::AgentOrderPlacement::Top,
    )
    .await
    .expect_err("failed provisional persistence must abort publication");

    assert!(!error.is_empty());
    assert!(state.agents.lock().await.is_empty());
    assert!(state.agent_order.lock().await.is_empty());
    assert!(completion.is_none());

    let cleanup = pending.stop_after_failure(error).await;
    assert!(!cleanup.is_empty());
    unsafe { std::env::set_var("WARDIAN_HOME", home.path()) };
}

#[tokio::test]
async fn provisional_commit_releases_roster_maps_while_barrier_is_busy() {
    let home = TestWardianHome::new_async().await;
    let state = Arc::new(AppState::new());
    let config = AgentConfig {
        session_id: "codex-provisional".into(),
        session_name: "codex-provisional".into(),
        provider: "codex".into(),
        ..Default::default()
    };
    let pending = super::PendingRuntime::prepare(&config, &state.terminal_sessions)
        .expect("reserve provisional stop registration")
        .attach(test_active_agent(&config.session_id));
    let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .expect("acquire test roster barrier")
        .expect("test roster barrier");
    let (barrier_ready, barrier_ready_rx) = tokio::sync::oneshot::channel();
    let (start_update, start_update_rx) = tokio::sync::oneshot::channel();
    let (update_entered, update_entered_rx) = tokio::sync::oneshot::channel();
    let (release_barrier, release_barrier_rx) = tokio::sync::oneshot::channel();
    let state_for_update = state.clone();
    let mut same_session_update = tokio::spawn(async move {
        barrier_ready.send(()).expect("barrier owner signal");
        start_update_rx.await.expect("start same-session update");
        update_entered
            .send(())
            .expect("same-session update entered");
        release_barrier_rx
            .await
            .expect("release roster barrier before update");
        let result = super::super::update_agent_from_control(
            &state_for_update,
            "codex-provisional",
            super::super::AgentControlUpdate {
                class: None,
                workspace: None,
                description: Some("same-session update while publishing"),
                model: None,
                reasoning_effort: None,
                classes: &[],
            },
        )
        .await;
        result.map(|(outcome, _)| outcome.config)
    });
    barrier_ready_rx.await.expect("same-session barrier owner");
    let mut completion = None;
    let mut commit = Box::pin(super::commit_registered_agent(
        &state,
        &config.session_id,
        pending,
        &mut completion,
        super::super::AgentOrderPlacement::Top,
    ));
    futures_util::future::poll_fn(|cx| match commit.as_mut().poll(cx) {
        std::task::Poll::Pending => std::task::Poll::Ready(()),
        std::task::Poll::Ready(_) => panic!("provisional commit unexpectedly completed"),
    })
    .await;
    assert!(state.agents.try_lock().is_ok());
    assert!(state.agent_order.try_lock().is_ok());
    assert!(
        state
            .try_lock_agent_lifecycle(&config.session_id)
            .await
            .is_some(),
        "provisional commit must release its lifecycle guard before waiting for the roster barrier"
    );

    let other = test_active_agent("claude-live");
    {
        let mut other_config = other.config.lock().expect("other config lock");
        other_config.provider = "claude".into();
        other_config.provider_config = ProviderConfig::Claude(Default::default());
    }
    state
        .agents
        .lock()
        .await
        .insert("claude-live".into(), other);
    state.agent_order.lock().await.push("claude-live".into());
    start_update.send(()).expect("start same-session update");
    update_entered_rx
        .await
        .expect("same-session update reached roster wait");
    drop(barrier);
    release_barrier
        .send(())
        .expect("release roster barrier before update");
    let (updated_config, committed) =
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let updated_config = (&mut same_session_update)
                .await
                .expect("same-session update task")
                .expect("same-session update");
            let committed = commit.await;
            (updated_config, committed)
        })
        .await
        .expect("same-session update and provisional commit must not deadlock");
    assert_eq!(
        updated_config.description,
        "same-session update while publishing"
    );
    assert!(
        committed.is_ok(),
        "provisional commit after barrier release failed"
    );
    let persisted: Vec<AgentConfig> = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("settings/state.json")).expect("state snapshot"),
    )
    .expect("state config JSON");
    assert_eq!(persisted.len(), 2);
    assert!(persisted.iter().any(|config| {
        config.session_id == "codex-provisional"
            && config.description == "same-session update while publishing"
    }));
    assert!(persisted
        .iter()
        .any(|config| config.session_id == "claude-live"));
}

#[tokio::test]
async fn detach_persists_current_roster_and_keeps_other_provider_changes_scoped() {
    let (home, state) = seeded_detach_state().await;
    let registration = crate::manager::codex_stop::prepare_stop(home.path(), "codex-provisional")
        .expect("test stop registration");
    let detached =
        match super::detach_provisional_codex(&state, "codex-provisional", 7, registration).await {
            Ok(Some(detached)) => detached,
            Ok(None) => panic!("current provisional runtime was missing"),
            Err(_) => panic!("detach should persist"),
        };

    {
        let mut agents = state.agents.lock().await;
        agents
            .get_mut("claude-live")
            .expect("other provider")
            .config
            .lock()
            .expect("config lock")
            .session_name = "renamed-while-cleaning".into();
    }
    super::persist_current_provisional_roster(&state)
        .await
        .expect("current roster snapshot");

    let persisted: Vec<AgentConfig> = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("settings/state.json")).expect("state snapshot"),
    )
    .expect("state config JSON");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].session_id, "claude-live");
    assert_eq!(persisted[0].session_name, "renamed-while-cleaning");
    assert!(!state.agents.lock().await.contains_key("codex-provisional"));

    detached
        .stop_guard
        .begin_stop()
        .wait()
        .await
        .expect("fake child exit");
}

#[tokio::test]
async fn failed_detach_persistence_retains_error_roster_and_stop_fence() {
    let (home, state) = seeded_detach_state().await;
    let settings = home.path().join("settings");
    std::fs::create_dir_all(&settings).expect("create settings directory");
    let barrier = wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
        .expect("acquire test roster barrier")
        .expect("test roster barrier available");
    drop(barrier);
    std::fs::create_dir(settings.join("state.json")).expect("block state destination");
    let registration = crate::manager::codex_stop::prepare_stop(home.path(), "codex-provisional")
        .expect("test stop registration");
    let error =
        match super::detach_provisional_codex(&state, "codex-provisional", 7, registration).await {
            Ok(_) => panic!("blocked state destination should fail publication"),
            Err(error) => error,
        };
    let super::DetachProvisionalError::Persistence {
        mut detached,
        error,
    } = error
    else {
        panic!("expected scoped persistence failure after roster barrier acquisition");
    };
    assert!(!error.is_empty());
    assert!(!state.agents.lock().await.contains_key("codex-provisional"));

    {
        let mut shell = detached
            .shell
            .as_ref()
            .expect("detached roster shell")
            .current_status
            .lock()
            .unwrap();
        *shell = "Error".into();
    }
    let retain_error =
        super::retain_failed_detached_roster(&state, "codex-provisional", &mut detached)
            .await
            .expect_err("blocked state destination should retain the failed entry in memory");
    assert!(!retain_error.is_empty());
    let agents = state.agents.lock().await;
    assert_eq!(
        agents
            .get("codex-provisional")
            .expect("failed provisional roster entry")
            .current_status
            .lock()
            .expect("status lock")
            .as_str(),
        "Error"
    );
    assert!(agents.contains_key("claude-live"));
    drop(agents);

    detached
        .stop_guard
        .begin_stop()
        .wait()
        .await
        .expect("retained fake stop fence");
}
