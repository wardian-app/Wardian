//! Agent lifecycle transition and rollback regressions.
use super::tests::{make_test_agent, WardianHomeGuard};
use super::*;

#[tokio::test]
async fn lock_agent_lifecycle_serializes_same_session() {
    let state = Arc::new(AppState::new());
    let first_guard = lock_agent_lifecycle(&state, "agent-1").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let state_for_task = Arc::clone(&state);

    let waiter = tokio::spawn(async move {
        let _second_guard = lock_agent_lifecycle(&state_for_task, "agent-1").await;
        tx.send(()).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(rx.try_recv().is_err());

    drop(first_guard);
    rx.recv()
        .await
        .expect("second lifecycle lock should acquire");
    waiter.await.unwrap();
}

#[test]
fn lifecycle_transition_lease_blocks_mutations_during_headless_execution() {
    let _lock = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().expect("temp wardian home");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    let config = AgentConfig {
        session_id: "agent-1".to_string(),
        session_name: "CoderOne".to_string(),
        provider: "mock".to_string(),
        resume_session: Some("provider-session-1".to_string()),
        ..Default::default()
    };
    let now = chrono::Utc::now();
    wardian_core::conversation_lease::acquire_lease(
        wardian_core::conversation_lease::ConversationLease {
            agent_id: config.session_id.clone(),
            provider: config.provider.clone(),
            resume_session: "provider-session-1".to_string(),
            owner_kind: "message_delivery".to_string(),
            owner_id: "interaction-1".to_string(),
            acquisition_id: "test-acquisition-1".to_string(),
            owner_node_id: None,
            mode: "background_resume".to_string(),
            started_at: now.to_rfc3339(),
            heartbeat_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
        },
        &now.to_rfc3339(),
    )
    .expect("headless lease");

    for operation in ["resume", "clear", "pause", "remove"] {
        let error = acquire_agent_lifecycle_transition_lease(&config, operation)
            .expect_err("lifecycle mutation must not overlap headless execution");
        assert!(error.contains("saved conversation is in use"), "{error}");
    }
}

#[test]
fn lifecycle_transition_lease_renewal_keeps_its_owner_active() {
    let _lock = crate::utils::wardian_test_env_lock();
    let temp = tempfile::tempdir().expect("temp wardian home");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    let config = AgentConfig {
        session_id: "agent-1".to_string(),
        session_name: "CoderOne".to_string(),
        provider: "mock".to_string(),
        ..Default::default()
    };
    let lease =
        acquire_agent_lifecycle_transition_lease(&config, "clear").expect("lifecycle lease");
    let owner = lease.owner().clone();
    let heartbeat_at = chrono::Utc::now() + chrono::Duration::minutes(1);

    assert!(
        renew_agent_lifecycle_transition_lease(&owner, heartbeat_at)
            .expect("renew lifecycle lease"),
        "the lifecycle heartbeat must not silently lose a current lease"
    );
    let leases = wardian_core::conversation_lease::load_leases();
    let active = wardian_core::conversation_lease::find_active_conflict(
        &leases,
        "agent-1",
        "",
        &heartbeat_at.to_rfc3339(),
    );
    assert!(
        active.is_some(),
        "renewed lifecycle lease should remain active"
    );
}

#[tokio::test]
async fn resume_and_clear_transfer_exact_lifecycle_lease_to_spawn_gate() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    for operation in ["resume", "clear"] {
        let original = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            session_name: format!("handoff-{operation}"),
            provider: "mock".to_string(),
            resume_session: Some("old-provider-session".to_string()),
            ..Default::default()
        };
        let lease = acquire_agent_lifecycle_transition_lease(&original, operation)
            .expect("lifecycle reservation");
        let owner = lease.owner().clone();
        let mut heartbeat = LifecycleLeaseHeartbeat::start(owner.clone());
        heartbeat.ensure_active(operation).expect("active owner");
        heartbeat.stop().await;

        let mut replacement = original.clone();
        replacement.resume_session = None;
        replacement.fresh_provider_session_id = Some("fresh-provider-session".to_string());
        let spawn_lease =
            crate::manager::spawn::provider_spawn_lease_for_launch(&replacement, Some(lease))
                .expect("same fenced owner enters the provider spawn boundary");
        assert_eq!(spawn_lease.owner(), &owner);
        assert!(
            acquire_agent_lifecycle_transition_lease(&replacement, "competing").is_err(),
            "{operation} must remain exclusive through the handoff"
        );
        assert!(wardian_core::conversation_lease::load_leases_checked()
            .expect("persisted owner")
            .iter()
            .any(|persisted| persisted.owner() == owner
                && persisted.resume_session == "fresh-provider-session"));

        drop(spawn_lease);
        acquire_agent_lifecycle_transition_lease(&replacement, "after-publication")
            .expect("release permits a later lifecycle operation");
    }
}

#[tokio::test]
async fn rotated_session_collision_blocks_inherited_spawn_at_launch_gate() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let original = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "mock".into(),
        resume_session: Some("old-session".into()),
        ..Default::default()
    };
    let inherited = acquire_agent_lifecycle_transition_lease(&original, "clear")
        .expect("original session lease");
    let mut other = original.clone();
    other.session_id = uuid::Uuid::new_v4().to_string();
    other.resume_session = Some("fresh-session".into());
    let other_guard = acquire_agent_lifecycle_transition_lease(&other, "other")
        .expect("other agent owns fresh session");

    let mut replacement = original;
    replacement.resume_session = None;
    replacement.fresh_provider_session_id = Some("fresh-session".into());
    let error =
        crate::manager::spawn::provider_spawn_lease_for_launch(&replacement, Some(inherited))
            .expect_err("fresh identity is owned by another agent");
    assert!(error.contains("saved conversation is leased"));
    assert!(wardian_core::conversation_lease::load_leases_checked()
        .expect("persisted lease")
        .iter()
        .any(|lease| lease.owner() == *other_guard.owner()));
}

#[test]
fn unpublished_replacement_failure_reports_bounded_retry_and_signals_watcher() {
    let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let error = retain_uncertain_replacement_until_expiry(&failed, "commit failed".into());
    assert!(failed.load(std::sync::atomic::Ordering::Acquire));
    assert!(error.contains("commit failed"));
    assert!(error.contains("until expiry"));
}

#[test]
fn failed_resume_after_old_runtime_teardown_cannot_restore_live_status() {
    let mut agent = make_test_agent();
    agent.config.lock().unwrap().session_id = "failed-resume".to_string();
    *agent.current_status.lock().unwrap() = "Idle".to_string();
    agent.runtime_generation = Some(7);
    let mut agents = HashMap::from([("failed-resume".to_string(), agent)]);

    let old_runtime = take_agent_runtime_for_termination(agents.get_mut("failed-resume").unwrap());
    assert_eq!(old_runtime.runtime_generation, Some(7));
    set_agent_status_after_failed_runtime_start(&mut agents, "failed-resume", "Error")
        .expect("failed runtime status");

    let retained = agents.get("failed-resume").unwrap();
    assert_eq!(*retained.current_status.lock().unwrap(), "Error");
    assert_eq!(retained.runtime_generation, None);
    assert_eq!(retained.process_id, None);
}

#[tokio::test]
async fn fresh_rotation_holds_old_session_before_runtime_stop() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("isolated lease home");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    let state = AppState::new();
    let mut agent = make_test_agent();
    agent.runtime_generation = Some(7);
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = "rotating-agent".into();
        config.provider = "mock".into();
        config.resume_session = Some("old-session".into());
    }
    state
        .agents
        .lock()
        .await
        .insert("rotating-agent".into(), agent);
    let lifecycle = super::acquire_agent_lifecycle_transition_lease_for_session(
        &state,
        "rotating-agent",
        "clear",
    )
    .await
    .expect("lifecycle exclusion");
    {
        let agents = state.agents.lock().await;
        super::hold_previous_provider_before_rotation(
            agents.get("rotating-agent").unwrap(),
            &lifecycle,
        )
        .expect("old session hold before stop");
    }
    drop(lifecycle);
    let leases = wardian_core::conversation_lease::load_leases_checked().unwrap();
    assert!(wardian_core::conversation_lease::find_active_conflict(
        &leases,
        "another-agent",
        "old-session",
        &chrono::Utc::now().to_rfc3339(),
    )
    .is_some());
    assert!(
        wardian_core::conversation_lease::find_active_execution_conflict(
            &leases,
            "another-agent",
            "old-session",
            &chrono::Utc::now().to_rfc3339(),
        )
        .is_none()
    );
}

#[tokio::test]
async fn lifecycle_transition_claims_the_persisted_lease_before_waiting_for_the_local_gate() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("temp wardian home");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    let state = AppState::new();
    let agent = make_test_agent();
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = "agent-1".to_string();
        config.session_name = "CoderOne".to_string();
        config.provider = "mock".to_string();
    }
    state
        .agents
        .lock()
        .await
        .insert("agent-1".to_string(), agent);

    let local_gate = lock_agent_lifecycle(&state, "agent-1").await;
    let lifecycle_lease = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        acquire_agent_lifecycle_transition_lease_for_session(&state, "agent-1", "resume"),
    )
    .await
    .expect("persisted lease acquisition must not wait for the local lifecycle gate")
    .expect("lifecycle lease");

    assert!(
        wardian_core::conversation_lease::find_active_conflict(
            &wardian_core::conversation_lease::load_leases(),
            "agent-1",
            "",
            &chrono::Utc::now().to_rfc3339(),
        )
        .is_some(),
        "the durable lease should be visible before the local gate is acquired"
    );
    drop(local_gate);
    drop(lifecycle_lease);
}

#[tokio::test]
async fn existing_lifecycle_guard_is_reused_without_self_deadlock() {
    let state = AppState::new();
    let guard = lock_agent_lifecycle(&state, "agent-1").await;

    let reused = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        acquire_agent_lifecycle_guard(&state, "agent-1", Some(guard)),
    )
    .await
    .expect("existing lifecycle guard should be reused");

    drop(reused);
}

#[test]
fn take_agent_runtime_for_termination_detaches_process_related_state() {
    let mut active = make_test_agent();
    active.runtime_generation = Some(7);
    active.process_id = Some(12345);

    let detached = take_agent_runtime_for_termination(&mut active);

    assert_eq!(detached.process_id, Some(12345));
    assert_eq!(detached.runtime_generation, Some(7));
    assert_eq!(active.process_id, None);
    assert_eq!(active.runtime_generation, None);
    assert!(active.child_process.is_none());
    assert!(active.background_processes.is_empty());
}

#[test]
fn prepare_agent_for_clear_preserves_boundary_evidence_until_replacement_commits() {
    let mut active = make_test_agent();
    active.runtime_generation = Some(9);
    active.process_id = Some(12345);
    let runtime_status = active.current_status.clone();
    *active.terminal_title.lock().unwrap() = "Old Title".to_string();
    *active.current_status.lock().unwrap() = "Idle".to_string();
    *active.query_count.lock().unwrap() = 5;
    *active.log_path.lock().unwrap() = Some(std::path::PathBuf::from("D:/tmp/agent.log"));
    *active.log_last_modified.lock().unwrap() = Some(std::time::SystemTime::now());
    *active.init_timestamp.lock().unwrap() = Some("2026-05-20T00:00:00Z".to_string());
    {
        let mut watch = active.watch_state.lock().unwrap();
        watch.push_output(b"old terminal output");
        watch.push_transcript(wardian_core::control::WatchTranscriptMessage {
            role: "assistant".to_string(),
            text: "old chat answer".to_string(),
            provider: "codex".to_string(),
            turn_id: Some("turn-before-clear".to_string()),
            source: Some("transcript".to_string()),
            provider_provenance: None,
        });
    }

    let mut prepared = prepare_agent_for_clear(&mut active);

    assert_eq!(prepared.termination.process_id, Some(12345));
    assert_eq!(
        prepared.config.session_id,
        active.config.lock().unwrap().session_id
    );
    assert_eq!(
        prepared.init_timestamp.as_deref(),
        Some("2026-05-20T00:00:00Z")
    );
    assert_eq!(active.process_id, None);
    assert_eq!(active.runtime_generation, None);
    assert!(
        !Arc::ptr_eq(&runtime_status, &active.current_status),
        "clear must detach stale runtime status writers before replacement spawn"
    );
    assert!(
        Arc::ptr_eq(&runtime_status, &prepared.termination.current_status),
        "the detached runtime must retain its original status Arc"
    );
    assert!(
        Arc::ptr_eq(&prepared.status_arc, &active.current_status),
        "the replacement status must be the incarnation installed in the agent map"
    );
    assert_eq!(active.terminal_title.lock().unwrap().as_str(), "Old Title");
    assert_eq!(
        active.current_status.lock().unwrap().as_str(),
        "Processing..."
    );
    assert_eq!(*active.query_count.lock().unwrap(), 5);
    assert_eq!(
        active.log_path.lock().unwrap().as_deref(),
        Some(std::path::Path::new("D:/tmp/agent.log"))
    );
    assert!(active.log_last_modified.lock().unwrap().is_some());
    let watch_snapshot = active
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .expect("watch snapshot after clear");
    assert!(watch_snapshot.output.text.contains("old terminal output"));
    assert_eq!(watch_snapshot.transcript.messages.len(), 1);
    assert_eq!(
        watch_snapshot.transcript.messages[0].text,
        "old chat answer"
    );

    let (restored_status, restored_same_runtime) =
        restore_agent_runtime_after_aborted_clear(&mut active, &mut prepared);
    assert!(restored_same_runtime);
    assert!(Arc::ptr_eq(&restored_status, &active.current_status));
    assert_eq!(active.runtime_generation, Some(9));
    assert_eq!(active.process_id, Some(12345));
    assert_eq!(active.current_status.lock().unwrap().as_str(), "Idle");
}

#[tokio::test]
async fn aborted_clear_releases_old_session_hold_after_exact_runtime_restore() {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let temp = tempfile::tempdir().expect("isolated lease home");
    std::env::set_var("WARDIAN_HOME", temp.path());
    let _home = WardianHomeGuard;
    let state = AppState::new();
    let mut agent = make_test_agent();
    agent.runtime_generation = Some(17);
    agent.process_id = Some(12345);
    {
        let mut config = agent.config.lock().unwrap();
        config.session_id = "clear-rollback".into();
        config.provider = "mock".into();
        config.resume_session = Some("old-clear-session".into());
    }
    state
        .agents
        .lock()
        .await
        .insert("clear-rollback".into(), agent);
    let lifecycle = super::acquire_agent_lifecycle_transition_lease_for_session(
        &state,
        "clear-rollback",
        "clear",
    )
    .await
    .expect("lifecycle exclusion");
    let hold = {
        let mut agents = state.agents.lock().await;
        let agent = agents.get_mut("clear-rollback").unwrap();
        let hold = super::hold_previous_provider_before_rotation(agent, &lifecycle)
            .expect("old-session hold")
            .expect("running runtime");
        let mut prepared = prepare_agent_for_clear(agent);
        let (status, same_runtime) =
            restore_agent_runtime_after_aborted_clear(agent, &mut prepared);
        assert!(Arc::ptr_eq(&status, &agent.current_status));
        assert!(same_runtime);
        hold
    };
    super::release_previous_hold_after_restored_clear(Some(&hold), false)
        .expect("uncertain restore retains hold");
    let leases = wardian_core::conversation_lease::load_leases_checked().unwrap();
    assert!(wardian_core::conversation_lease::find_active_conflict(
        &leases,
        "another-agent",
        "old-clear-session",
        &chrono::Utc::now().to_rfc3339(),
    )
    .is_some());
    super::release_previous_hold_after_restored_clear(Some(&hold), true)
        .expect("exact restored hold release");
    drop(lifecycle);
    let leases = wardian_core::conversation_lease::load_leases_checked().unwrap();
    assert!(wardian_core::conversation_lease::find_active_conflict(
        &leases,
        "another-agent",
        "old-clear-session",
        &chrono::Utc::now().to_rfc3339(),
    )
    .is_none());
}

#[test]
fn runtime_replacement_status_incarnation_rejects_late_runtime_writers() {
    let mut active = make_test_agent();
    let old_status = active.current_status.clone();

    let replacement_status = replace_agent_status_incarnation(&mut active, "Off");

    assert!(
        !Arc::ptr_eq(&old_status, &active.current_status),
        "late events must keep the old status Arc"
    );
    assert!(Arc::ptr_eq(&replacement_status, &active.current_status));
    assert_eq!(replacement_status.lock().unwrap().as_str(), "Off");
}
