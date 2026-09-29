include!("opencode_startup_tests.rs");

use super::*;
use rusqlite::Connection;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

fn test_snapshot(status: &str) -> AgentSnapshot {
    AgentSnapshot {
        session_id: "agent-1".to_string(),
        provider: "opencode".to_string(),
        folder: "D:/work".to_string(),
        is_off: false,
        resume_session: None,
        provider_generation: 0,
        process_id: Some(1234),
        query_count: Arc::new(Mutex::new(0)),
        init_timestamp: Arc::new(Mutex::new(None)),
        last_query_timestamp: Arc::new(Mutex::new(None)),
        current_status: Arc::new(Mutex::new(status.to_string())),
        status_observation: Mutex::new(TelemetryStatusDraft {
            initial_status: status.to_string(),
            current_status: status.to_string(),
            initial_status_revision: 0,
            initial_status_intent_revision: 0,
            transitions: Vec::new(),
        }),
        watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            1024,
        ))),
        last_output_at: Arc::new(Mutex::new(None)),
        log_path: Arc::new(Mutex::new(None)),
        log_last_modified: Arc::new(Mutex::new(None)),
    }
}

fn test_active_agent(
    session_id: &str,
    provider: &str,
    status: &str,
    process_id: Option<u32>,
) -> crate::state::ActiveAgent {
    crate::state::ActiveAgent {
        config: Arc::new(Mutex::new(wardian_core::models::AgentConfig {
            session_id: session_id.to_string(),
            provider: provider.to_string(),
            ..Default::default()
        })),
        child_process: None,
        background_processes: Vec::new(),
        memory_capability: None,
        runtime_generation: None,
        process_id,
        query_count: Arc::new(Mutex::new(0)),
        init_timestamp: Arc::new(Mutex::new(None)),
        last_query_timestamp: Arc::new(Mutex::new(None)),
        current_status: Arc::new(Mutex::new(status.to_string())),
        last_status_at: Arc::new(Mutex::new(None)),
        watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            session_id.to_string(),
            16,
            1024,
        ))),
        terminal_title: Arc::new(Mutex::new(String::new())),
        last_output_at: Arc::new(Mutex::new(None)),
        log_path: Arc::new(Mutex::new(None)),
        log_last_modified: Arc::new(Mutex::new(None)),
        #[cfg(windows)]
        job_object: None,
    }
}

fn test_snapshot_from_agent(
    agent: &crate::state::ActiveAgent,
    provider_generation: u64,
    state: &crate::state::AppState,
) -> AgentSnapshot {
    let config = agent.config.lock().unwrap();
    let snapshot = AgentSnapshot {
        session_id: config.session_id.clone(),
        provider: config.provider.clone(),
        folder: config.folder.clone(),
        is_off: config.is_off,
        resume_session: super::opencode_telemetry_session_id(&config),
        provider_generation,
        process_id: agent.process_id,
        query_count: agent.query_count.clone(),
        init_timestamp: agent.init_timestamp.clone(),
        last_query_timestamp: agent.last_query_timestamp.clone(),
        current_status: agent.current_status.clone(),
        status_observation: Mutex::new(TelemetryStatusDraft::default()),
        watch_state: agent.watch_state.clone(),
        last_output_at: agent.last_output_at.clone(),
        log_path: agent.log_path.clone(),
        log_last_modified: agent.log_last_modified.clone(),
    };
    snapshot.capture_initial_status(state);
    snapshot
}

#[test]
fn cached_provider_query_timestamp_survives_an_unchanged_log_pass() {
    let snap = test_snapshot("Idle");
    *snap.last_query_timestamp.lock().unwrap() = Some("2026-05-14T12:00:03.000Z".to_string());
    let mut latest = Some("2026-05-14T12:00:01.000Z".to_string());

    super::reconcile_cached_last_query_timestamp(&mut latest, &snap.last_query_timestamp);

    assert_eq!(latest.as_deref(), Some("2026-05-14T12:00:03.000Z"));
    assert_eq!(
        snap.last_query_timestamp.lock().unwrap().as_deref(),
        Some("2026-05-14T12:00:03.000Z")
    );
}

#[test]
fn stopped_agents_reconcile_provider_logs_even_with_durable_queries() {
    assert!(super::should_run_provider_log_telemetry("Off", Some(false)));
}

#[test]
fn antigravity_wal_activity_advances_the_telemetry_watermark() {
    let temp = tempfile::tempdir().expect("temp dir");
    let database = temp.path().join("conversation.db");
    let writer = Connection::open(&database).expect("open database");
    writer
        .execute_batch(
            "PRAGMA journal_mode = WAL;
                 CREATE TABLE steps (idx INTEGER, step_type INTEGER, metadata BLOB);
                 INSERT INTO steps (idx, step_type) VALUES (1, 14);",
        )
        .expect("create WAL fixture");
    let before = super::telemetry_source_modified("antigravity", &database)
        .expect("initial database watermark");

    std::thread::sleep(std::time::Duration::from_millis(20));
    writer
        .execute(
            "INSERT INTO steps (idx, step_type) VALUES (?1, ?2)",
            rusqlite::params![2_i64, 14_i64],
        )
        .expect("append WAL user message");

    assert!(database.with_file_name("conversation.db-wal").exists());
    let after = super::telemetry_source_modified("antigravity", &database)
        .expect("updated database watermark");
    assert!(after > before, "WAL activity must invalidate the cache");
}

#[test]
fn restart_hydration_recovers_a_user_timestamp_before_a_large_jsonl_tail() {
    let temp = tempfile::tempdir().expect("temp dir");
    let log = temp.path().join("rollout.jsonl");
    let user_message = serde_json::json!({
        "type": "event_msg",
        "timestamp": "2026-08-26T12:00:00.000Z",
        "payload": { "type": "user_message", "message": "hello" },
    });
    let large_assistant_record = serde_json::json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "assistant",
            "content": "x".repeat((super::LOG_PARSE_TAIL_BYTES + 1024) as usize),
        },
    });
    std::fs::write(
        &log,
        format!(
            "{}\n{}\n",
            user_message,
            serde_json::to_string(&large_assistant_record).expect("serialize assistant")
        ),
    )
    .expect("write oversized provider log");

    assert!(std::fs::metadata(&log).expect("log metadata").len() > super::LOG_PARSE_TAIL_BYTES);
    assert_eq!(
        super::latest_query_timestamp_from_log_suffix(&log, "codex").as_deref(),
        Some("2026-08-26T12:00:00.000Z")
    );
}

#[test]
fn normalizes_process_tree_cpu_to_whole_machine_capacity() {
    assert_eq!(super::normalize_cpu_usage(260.0, 4), 65.0);
    assert_eq!(super::normalize_cpu_usage(800.0, 4), 100.0);
    assert_eq!(super::normalize_cpu_usage(-5.0, 4), 0.0);
}

#[test]
fn treats_missing_cpu_count_as_single_cpu() {
    assert_eq!(super::normalize_cpu_usage(260.0, 0), 100.0);
}

#[test]
fn converts_resident_bytes_to_mib() {
    assert_eq!(super::bytes_to_mib(1_048_576), 1.0);
    assert_eq!(super::bytes_to_mib(2_621_440), 2.5);
}

#[cfg(windows)]
#[test]
fn tracked_process_refresh_reuses_inventory_and_costs_less_than_full_scan() {
    let mut cache = super::process_inventory_cache().lock().unwrap();
    *cache = None;
    drop(cache);

    let system = tokio::sync::Mutex::new(sysinfo::System::new());
    let session_ids = (0..58)
        .map(|index| format!("agent-{index}"))
        .collect::<Vec<_>>();
    let agent_roots = session_ids
        .iter()
        .cloned()
        .map(|session_id| (session_id, Some(std::process::id())))
        .collect::<Vec<_>>();

    let full = super::refresh_system_process_snapshot(&system, &session_ids, &agent_roots)
        .expect("full inventory refresh should succeed");
    let tracked = super::refresh_system_process_snapshot(&system, &session_ids, &agent_roots)
        .expect("tracked refresh should succeed");

    assert!(std::sync::Arc::ptr_eq(
        &full.children_map,
        &tracked.children_map
    ));
    assert!(tracked.processes.contains_key(&std::process::id()));
    eprintln!(
        "telemetry process refresh: full={:?}, tracked={:?}",
        full.sys_refresh, tracked.sys_refresh
    );
    assert!(tracked.sys_refresh < full.sys_refresh);
}

#[test]
fn process_inventory_agent_key_is_order_independent() {
    let left = super::process_inventory_agent_key(&[
        ("agent-2".to_string(), Some(2)),
        ("agent-1".to_string(), Some(1)),
    ]);
    let right = super::process_inventory_agent_key(&[
        ("agent-1".to_string(), Some(1)),
        ("agent-2".to_string(), Some(2)),
    ]);

    assert_eq!(left, right);
}

#[cfg(windows)]
#[test]
fn changing_agent_process_id_does_not_force_marker_discovery() {
    let session_id = "pid-churn-marker-discovery-test".to_string();
    let session_ids = vec![session_id.clone()];
    let cached_markers = HashMap::from([(session_id.clone(), vec![12345])]);

    *super::process_inventory_cache().lock().unwrap() = None;
    *super::session_roots_cache().lock().unwrap() = Some(super::SessionRootsCache {
        roots: cached_markers.clone(),
        refreshed_at: std::time::Instant::now(),
        session_key: super::sorted_session_key(&session_ids),
    });

    let system = tokio::sync::Mutex::new(sysinfo::System::new());
    super::refresh_system_process_snapshot(
        &system,
        &session_ids,
        &[(session_id.clone(), Some(101))],
    )
    .expect("initial inventory refresh should succeed");

    // Re-seed the marker cache so the assertion observes whether the PID
    // change caused a second marker scan, rather than its initial setup.
    *super::session_roots_cache().lock().unwrap() = Some(super::SessionRootsCache {
        roots: cached_markers.clone(),
        refreshed_at: std::time::Instant::now(),
        session_key: super::sorted_session_key(&session_ids),
    });

    super::refresh_system_process_snapshot(&system, &session_ids, &[(session_id, Some(202))])
        .expect("PID-churn inventory refresh should succeed");

    assert_eq!(super::cached_session_roots(), cached_markers);
}

#[test]
fn collects_root_descendants_and_discovered_session_roots_without_duplicates() {
    let children_map = HashMap::from([(1, vec![2, 4]), (2, vec![3]), (4, vec![5]), (9, vec![10])]);

    let related = super::collect_related_pids(Some(1), &[2, 9], &children_map);

    assert_eq!(related, BTreeSet::from([1_u32, 2, 3, 4, 5, 9, 10]));
}

#[test]
fn app_process_pids_exclude_agent_trees_to_prevent_double_counting() {
    let children_map = HashMap::from([
        (1, vec![2, 3, 6]),
        (3, vec![4, 5]),
        (6, vec![7]),
        (8, vec![9]),
    ]);

    let app_pids = super::collect_app_process_pids(1, &[3, 7, 8], &children_map);

    assert_eq!(app_pids, BTreeSet::from([1_u32, 2, 6]));
}

#[cfg(windows)]
#[test]
fn discovers_session_roots_for_multiple_agents_from_one_process_marker_snapshot() {
    let markers = vec![
        super::ProcessMarkerSnapshot {
            pid: 10,
            process_name: "cmd.exe".to_string(),
            command_line: "cmd.exe /d /c codex.cmd resume session-a --cd D:/repo".to_string(),
            environ: Vec::new(),
        },
        super::ProcessMarkerSnapshot {
            pid: 11,
            process_name: "node.exe".to_string(),
            command_line: "node codex".to_string(),
            environ: vec!["WARDIAN_SESSION_ID=session-a".to_string()],
        },
        super::ProcessMarkerSnapshot {
            pid: 20,
            process_name: "node.exe".to_string(),
            command_line: "node other".to_string(),
            environ: vec!["WARDIAN_SESSION_ID=session-b".to_string()],
        },
        super::ProcessMarkerSnapshot {
            pid: 30,
            process_name: "pwsh.exe".to_string(),
            command_line: "pwsh -NoLogo".to_string(),
            environ: Vec::new(),
        },
    ];

    let roots = super::discover_session_roots_from_process_markers(
        &["session-a".to_string(), "session-b".to_string()],
        &markers,
    );

    assert_eq!(roots["session-a"], vec![10, 11]);
    assert_eq!(roots["session-b"], vec![20]);
}

/// A Processing agent whose provider log has just staged Idle, with a durable
/// row so the transition can commit.
async fn processing_agent_that_staged_idle(
    state: &crate::state::AppState,
    session_id: &str,
) -> super::TelemetryProviderStatus {
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id,
        session_name: "test-agent",
        description: "",
        agent_class: "Coder",
        provider: "claude",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("insert isolated persisted agent");
    let agent = test_active_agent(session_id, "claude", "Processing...", Some(1234));
    wardian_core::db::update_agent_status(session_id, "Processing...", Some(1234))
        .expect("persist initial status");
    let snap = test_snapshot_from_agent(&agent, 0, state);
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), agent);
    super::set_snapshot_status(&snap, "Idle");
    snap.provider_status_observation(false)
}

#[tokio::test]
async fn a_held_lifecycle_gate_defers_the_observation_instead_of_stalling_or_losing_it() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let session_id = "agent-1";
    let state = crate::state::AppState::new();
    let observation = processing_agent_that_staged_idle(&state, session_id).await;
    let mut metrics = vec![super::AgentTelemetry {
        session_id: session_id.to_string(),
        cpu_usage: 0.0,
        memory_mb: 0.0,
        uptime_seconds: 0,
        query_count: 0,
        init_timestamp: None,
        last_query_timestamp: None,
        current_status: "Idle".to_string(),
        log_path: None,
    }];

    // New Session, restart, restore, and even a notification hold this gate.
    let lifecycle = state.lock_agent_lifecycle(session_id).await;
    let follow_up = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        super::apply_provider_status_observations(
            &state,
            std::slice::from_ref(&observation),
            &mut metrics,
        ),
    )
    .await
    .expect("the metrics tick must not wait out a lifecycle operation");

    assert!(follow_up.wake_sessions.is_empty());
    assert_eq!(follow_up.deferred.len(), 1);
    assert_eq!(
        metrics[0].current_status, "Processing...",
        "a deferred observation shows the runtime's own status"
    );
    assert_eq!(
        state
            .agents
            .lock()
            .await
            .get(session_id)
            .map(|agent| agent.current_status.lock().unwrap().clone())
            .as_deref(),
        Some("Processing..."),
        "a deferred observation must not commit while the gate is held"
    );

    // The staged transition survives: once the gate frees it publishes.
    drop(lifecycle);
    let publication =
        crate::manager::publish_telemetry_status_observation(&state, &follow_up.deferred[0]).await;
    assert_eq!(
        publication.readiness,
        Some(wardian_core::control::ProviderInputReadiness::Ready)
    );
    assert_eq!(publication.current_status.as_deref(), Some("Idle"));
}

#[tokio::test]
async fn an_observation_without_a_staged_transition_is_not_deferred() {
    let state = crate::state::AppState::new();
    let session_id = "agent-1";
    let agent = test_active_agent(session_id, "claude", "Processing...", Some(1234));
    let snap = test_snapshot_from_agent(&agent, 0, &state);
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), agent);
    let observation = snap.provider_status_observation(false);
    assert!(observation.transitions.is_empty());

    let _lifecycle = state.lock_agent_lifecycle(session_id).await;
    let follow_up = super::apply_provider_status_observations(
        &state,
        std::slice::from_ref(&observation),
        &mut [],
    )
    .await;

    assert!(follow_up.deferred.is_empty());
}

#[tokio::test]
async fn ready_transition_is_reported_for_wakeup_instead_of_delivered_inline() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let session_id = "agent-1";
    let state = crate::state::AppState::new();
    let observation = processing_agent_that_staged_idle(&state, session_id).await;

    let follow_up = super::apply_provider_status_observations(
        &state,
        std::slice::from_ref(&observation),
        &mut [],
    )
    .await;

    assert_eq!(follow_up.wake_sessions, vec![session_id.to_string()]);
    assert!(follow_up.deferred.is_empty());
}

#[tokio::test]
async fn current_runtime_telemetry_status_commit_persists_and_records_watch_event() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let session_id = "agent-1";
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id,
        session_name: "test-agent",
        description: "",
        agent_class: "Coder",
        provider: "claude",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("insert isolated persisted agent");

    let state = crate::state::AppState::new();
    let agent = test_active_agent(session_id, "claude", "Processing...", Some(1234));
    wardian_core::db::update_agent_status(session_id, "Processing...", Some(1234))
        .expect("persist initial runtime status");
    let snap = test_snapshot_from_agent(&agent, 0, &state);
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), agent);

    super::set_snapshot_status(&snap, "Idle");
    assert_eq!(
        *snap.current_status.lock().unwrap(),
        "Processing...",
        "detached telemetry stages status without mutating shared runtime state"
    );
    let publication = crate::manager::publish_telemetry_status_observation(
        &state,
        &snap.provider_status_observation(false),
    )
    .await;

    assert_eq!(
        publication.readiness,
        Some(wardian_core::control::ProviderInputReadiness::Ready)
    );
    assert_eq!(publication.current_status.as_deref(), Some("Idle"));
    let agents = state.agents.lock().await;
    let active = agents.get(session_id).expect("active runtime");
    assert_eq!(*active.current_status.lock().unwrap(), "Idle");
    assert!(active.last_status_at.lock().unwrap().is_some());
    let watch_events = active
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .unwrap()
        .events;
    assert!(watch_events.iter().any(|event| {
        event.kind == "status"
            && event.payload.get("status").and_then(|value| value.as_str()) == Some("idle")
    }));
    drop(agents);

    let (database_status, database_pid) = wardian_core::db::get_db_conn(|conn| {
        Ok(conn.query_row(
            "SELECT last_status, last_pid FROM agents WHERE session_id = ?1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )?)
    })
    .expect("read isolated persisted status");
    assert_eq!(database_status, "Idle");
    assert_eq!(database_pid, Some(1234));
}

#[tokio::test]
async fn terminal_status_intent_rejects_stale_idle_and_processing_snapshots() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let state = crate::state::AppState::new();

    for (session_id, terminal_status, stale_status, newer_matching_intent) in [
        ("agent-off-idle", "Off", "Idle", false),
        ("agent-error-processing", "Error", "Processing...", false),
        ("agent-action-idle", "Action Needed", "Idle", false),
        ("agent-error-new-idle-intent", "Error", "Idle", true),
    ] {
        wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
            session_id,
            session_name: session_id,
            description: "",
            agent_class: "Coder",
            provider: "codex",
            workspace: None,
            project: None,
            is_off: terminal_status == "Off",
            created_at: None,
        })
        .expect("insert isolated persisted agent");

        let agent = test_active_agent(session_id, "codex", terminal_status, Some(1234));
        agent.config.lock().unwrap().is_off = terminal_status == "Off";
        agent
            .watch_state
            .lock()
            .unwrap()
            .set_codex_attachment_ready(true);
        let current_status = agent.current_status.clone();
        state.reserve_status_intent(session_id, &current_status, terminal_status);
        state.commit_status_revision(session_id, &current_status, terminal_status);
        wardian_core::db::update_agent_status(session_id, terminal_status, Some(1234))
            .expect("persist current terminal status");
        let snapshot = test_snapshot_from_agent(&agent, 0, &state);
        state
            .agents
            .lock()
            .await
            .insert(session_id.to_string(), agent);
        if newer_matching_intent {
            let _current = current_status.lock().unwrap();
            state.reserve_status_intent(session_id, &current_status, stale_status);
        }

        // Log-derived status can lag the already-current terminal intent, so
        // this detached snapshot must not publish its stale ready or busy inference.
        super::set_snapshot_status(&snapshot, stale_status);
        let observation = snapshot.provider_status_observation(false);
        let mut metrics = vec![super::AgentTelemetry {
            session_id: session_id.to_string(),
            cpu_usage: 0.0,
            memory_mb: 0.0,
            uptime_seconds: 0,
            query_count: 0,
            init_timestamp: None,
            last_query_timestamp: None,
            current_status: stale_status.to_string(),
            log_path: None,
        }];
        super::apply_provider_status_observations(
            &state,
            std::slice::from_ref(&observation),
            &mut metrics,
        )
        .await;

        let expected_status = if newer_matching_intent {
            stale_status
        } else {
            terminal_status
        };
        assert_eq!(
            *current_status.lock().unwrap(),
            expected_status,
            "telemetry published the wrong status for {session_id}"
        );
        let database_status = wardian_core::db::get_db_conn(|conn| {
            Ok(conn.query_row(
                "SELECT last_status FROM agents WHERE session_id = ?1",
                [session_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .expect("read isolated persisted status");
        assert_eq!(
            database_status, expected_status,
            "telemetry persisted the wrong status for {session_id}"
        );
        assert_eq!(
            state.remote_agent_status(session_id).as_deref(),
            newer_matching_intent.then_some(stale_status)
        );
        assert_eq!(
            state
                .interactions
                .provider_input_state(session_id)
                .await
                .is_some(),
            newer_matching_intent
        );

        let agents = state.agents.lock().await;
        let events = agents[session_id]
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, None)
            .unwrap()
            .events;
        let observed_stale_status = events.iter().any(|event| {
            event.payload.get("status").and_then(|value| value.as_str())
                == Some(wardian_core::identity::normalize_status(stale_status).as_str())
        });
        assert_eq!(observed_stale_status, newer_matching_intent);
    }
}

#[tokio::test]
async fn telemetry_status_aba_transition_does_not_commit_stale_log_state() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let session_id = "agent-1";
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id,
        session_name: "test-agent",
        description: "",
        agent_class: "Coder",
        provider: "claude",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("insert isolated persisted agent");

    let state = crate::state::AppState::new();
    let agent = test_active_agent(session_id, "claude", "Idle", Some(1234));
    let current_status = agent.current_status.clone();
    wardian_core::db::update_agent_status(session_id, "Idle", Some(1234))
        .expect("persist initial status");
    let snapshot = test_snapshot_from_agent(&agent, 0, &state);
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), agent);

    super::set_snapshot_status(&snapshot, "Processing...");
    {
        // Match the live setter's atomic status-and-sequence ordering while
        // returning to the snapshot's original string to exercise ABA.
        let mut status = current_status.lock().unwrap();
        *status = "Processing...".to_string();
        state.commit_status_revision(session_id, &current_status, "Processing...");
        state.next_status_observation_sequence(session_id);
        *status = "Idle".to_string();
        state.commit_status_revision(session_id, &current_status, "Idle");
        state.next_status_observation_sequence(session_id);
    }

    let publication = crate::manager::publish_telemetry_status_observation(
        &state,
        &snapshot.provider_status_observation(false),
    )
    .await;

    assert_eq!(publication.readiness, None);
    assert_eq!(publication.current_status.as_deref(), Some("Idle"));
    assert_eq!(*current_status.lock().unwrap(), "Idle");
    let database_status = wardian_core::db::get_db_conn(|conn| {
        Ok(conn.query_row(
            "SELECT last_status FROM agents WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, String>(0),
        )?)
    })
    .expect("read isolated status");
    assert_eq!(database_status, "Idle");
    assert!(state.remote_agent_status(session_id).is_none());
    let agents = state.agents.lock().await;
    let events = agents[session_id]
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .unwrap()
        .events;
    assert!(events.iter().all(|event| {
        event.payload.get("status").and_then(|value| value.as_str()) != Some("processing")
    }));
}

#[tokio::test]
async fn stale_telemetry_ready_cannot_overtake_a_newer_busy_observation() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let session_id = "agent-1";
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id,
        session_name: "test-agent",
        description: "",
        agent_class: "Coder",
        provider: "claude",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("insert isolated persisted agent");

    let state = crate::state::AppState::new();
    let agent = test_active_agent(session_id, "claude", "Processing...", Some(1234));
    let current_status = agent.current_status.clone();
    wardian_core::db::update_agent_status(session_id, "Processing...", Some(1234))
        .expect("persist initial status");
    let snapshot = test_snapshot_from_agent(&agent, 1, &state);
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), agent);
    super::set_snapshot_status(&snapshot, "Idle");
    let observation = snapshot.provider_status_observation(false);
    let publication =
        crate::manager::publish_telemetry_status_observation(&state, &observation).await;
    assert_eq!(
        publication.readiness,
        Some(wardian_core::control::ProviderInputReadiness::Ready)
    );
    let ready_revision = publication
        .status_revision
        .expect("committed status revision");

    let barrier = tokio::sync::Barrier::new(2);
    let busy_observation = async {
        let busy_revision = {
            let mut status = current_status.lock().unwrap();
            *status = "Processing...".to_string();
            state.commit_status_revision(session_id, &current_status, "Processing...")
        };
        let (_, became_ready) = state
            .interactions
            .record_provider_input_status_observation_with_transition(
                session_id,
                busy_revision,
                observation.generation,
                wardian_core::control::ProviderInputReadiness::Busy,
                None,
            )
            .await;
        assert!(!became_ready);
        barrier.wait().await;
    };
    let stale_telemetry = async {
        barrier.wait().await;
        assert!(ready_revision < state.status_revision(session_id, &current_status));
        super::apply_telemetry_provider_readiness(&state, &observation, &publication).await
    };
    let (_, dispatched) = tokio::join!(busy_observation, stale_telemetry);

    assert!(!dispatched, "stale Ready must not dispatch queued work");
    let provider_input = state
        .interactions
        .provider_input_state(session_id)
        .await
        .expect("newer Busy input state");
    assert_eq!(
        provider_input.state,
        wardian_core::control::ProviderInputReadiness::Busy
    );
    assert_eq!(provider_input.ready_evidence, None);
}

#[tokio::test]
async fn telemetry_ready_evidence_survives_a_same_status_idle_intent() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let session_id = "agent-1";
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id,
        session_name: "test-agent",
        description: "",
        agent_class: "Coder",
        provider: "claude",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("insert isolated persisted agent");

    let state = crate::state::AppState::new();
    let agent = test_active_agent(session_id, "claude", "Processing...", Some(1234));
    let current_status = agent.current_status.clone();
    wardian_core::db::update_agent_status(session_id, "Processing...", Some(1234))
        .expect("persist initial status");
    let snapshot = test_snapshot_from_agent(&agent, 1, &state);
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), agent);

    super::set_snapshot_status(&snapshot, "Idle");
    let observation = snapshot.provider_status_observation(false);
    let publication =
        crate::manager::publish_telemetry_status_observation(&state, &observation).await;
    assert_eq!(
        publication.readiness,
        Some(wardian_core::control::ProviderInputReadiness::Ready)
    );
    let committed_revision = publication
        .status_revision
        .expect("committed telemetry revision");

    let intent_revision = crate::manager::reserve_agent_status_intent(
        &state,
        crate::manager::codex_onboarding::CodexStatusAdmission::Allowed,
        session_id,
        &current_status,
        "Idle",
        "Idle",
    )
    .expect("same-status intent should be accepted");
    assert_eq!(
        state.status_intent_revision(session_id, &current_status),
        intent_revision
    );
    assert_eq!(
        state.status_revision(session_id, &current_status),
        committed_revision,
        "same-status intent must not invalidate an unchanged committed status"
    );

    assert!(
        super::apply_telemetry_provider_readiness(&state, &observation, &publication).await,
        "previously staged Ready evidence should still apply after the Idle no-op"
    );
    let provider_input = state
        .interactions
        .provider_input_state(session_id)
        .await
        .expect("provider input state");
    assert_eq!(
        provider_input.state,
        wardian_core::control::ProviderInputReadiness::Ready
    );
    assert!(provider_input.ready_evidence.is_some());
}

#[test]
fn telemetry_status_noop_does_not_emit_duplicate_watch_event() {
    let snap = test_snapshot("Idle");

    super::set_snapshot_status(&snap, "Idle");

    assert_eq!(snap.telemetry_status(), "Idle");
    assert!(snap
        .status_observation
        .lock()
        .unwrap()
        .transitions
        .is_empty());
    let snapshot = snap
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .unwrap();
    assert!(snapshot.events.is_empty());
}

#[test]
fn telemetry_status_reconciliation_preserves_headless_execution_projection() {
    assert_eq!(super::telemetry_display_status("Off", true), "Headless");
    assert_eq!(super::telemetry_display_status("Error", true), "Headless");
    assert_eq!(
        super::telemetry_display_status("Processing...", true),
        "Processing..."
    );
    assert_eq!(super::telemetry_display_status("Off", false), "Off");
}

#[tokio::test]
async fn codex_telemetry_status_commit_rechecks_attachment_gate() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let session_id = "agent-1";
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id,
        session_name: "test-agent",
        description: "",
        agent_class: "Coder",
        provider: "codex",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("insert isolated persisted agent");

    let state = crate::state::AppState::new();
    let agent = test_active_agent(session_id, "codex", "Processing...", Some(1234));
    let watch_state = agent.watch_state.clone();
    wardian_core::db::update_agent_status(session_id, "Processing...", Some(1234))
        .expect("persist initial runtime status");
    let snap = test_snapshot_from_agent(&agent, 1, &state);
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), agent);

    // Stage while the captured runtime is attached, then close the live
    // gate before publication to model an attachment becoming provisional.
    super::set_snapshot_status(&snap, "Idle");
    watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(false);
    let publication = crate::manager::publish_telemetry_status_observation(
        &state,
        &snap.provider_status_observation(false),
    )
    .await;

    assert_eq!(
        publication.readiness,
        Some(wardian_core::control::ProviderInputReadiness::Unknown)
    );
    assert_eq!(publication.current_status.as_deref(), Some("Processing..."));
    let database_status = wardian_core::db::get_db_conn(|conn| {
        Ok(conn.query_row(
            "SELECT last_status FROM agents WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, String>(0),
        )?)
    })
    .expect("read isolated persisted status");
    assert_eq!(database_status, "Processing...");
    let active_watch = state.agents.lock().await;
    let events = active_watch
        .get(session_id)
        .unwrap()
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .unwrap()
        .events;
    assert!(events.iter().all(|event| {
        event.payload.get("status").and_then(|value| value.as_str()) != Some("idle")
    }));
}

#[tokio::test]
async fn replaced_codex_telemetry_snapshot_cannot_persist_idle_before_attachment_gate() {
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let session_id = "agent-1";
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id,
        session_name: "test-agent",
        description: "",
        agent_class: "Coder",
        provider: "codex",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("insert isolated persisted agent");

    let state = crate::state::AppState::new();
    let old_agent = test_active_agent(session_id, "codex", "Processing...", Some(1111));
    let old_snapshot = test_snapshot_from_agent(&old_agent, 493, &state);
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), old_agent);
    wardian_core::db::update_agent_status(session_id, "Processing...", Some(1111))
        .expect("persist old runtime status");

    let new_agent = test_active_agent(session_id, "codex", "Starting", Some(2222));
    new_agent
        .watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(false);
    new_agent
        .watch_state
        .lock()
        .unwrap()
        .push_event("status", serde_json::json!({"status": "starting"}));
    state
        .agents
        .lock()
        .await
        .insert(session_id.to_string(), new_agent);
    wardian_core::db::update_agent_status(session_id, "Starting", Some(2222))
        .expect("persist replacement runtime status");

    // Model a telemetry worker applying a status from its detached old
    // snapshot after the replacement runtime has entered the roster.
    super::set_snapshot_status(&old_snapshot, "Idle");
    let observation = old_snapshot.provider_status_observation(false);
    let mut metrics = vec![super::AgentTelemetry {
        session_id: session_id.to_string(),
        cpu_usage: 0.0,
        memory_mb: 0.0,
        uptime_seconds: 0,
        query_count: 0,
        init_timestamp: None,
        last_query_timestamp: None,
        current_status: "Idle".to_string(),
        log_path: None,
    }];
    super::apply_provider_status_observations(
        &state,
        std::slice::from_ref(&observation),
        &mut metrics,
    )
    .await;
    assert_eq!(metrics[0].current_status, "Starting");
    assert!(state
        .interactions
        .provider_input_state(session_id)
        .await
        .is_none());
    assert_eq!(state.remote_agent_status(session_id), None);
    let agents = state.agents.lock().await;
    let current_agent = agents
        .get(session_id)
        .expect("replacement agent remains live");
    assert_eq!(
        *current_agent.current_status.lock().unwrap(),
        "Starting",
        "the live replacement status must stay Starting"
    );
    let current_events = current_agent
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, None)
        .unwrap()
        .events;
    assert!(
        current_events.iter().all(|event| {
            event.payload.get("status").and_then(|value| value.as_str()) != Some("idle")
        }),
        "the replacement watch stream must not receive stale Idle"
    );
    drop(agents);

    let (database_status, database_pid) = wardian_core::db::get_db_conn(|conn| {
        Ok(conn.query_row(
            "SELECT last_status, last_pid FROM agents WHERE session_id = ?1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )?)
    })
    .expect("read isolated persisted status");
    assert_eq!(database_status, "Starting");
    assert_eq!(database_pid, Some(2222));

    let status_events = wardian_core::db::get_db_conn(|conn| {
            let mut statement = conn.prepare(
                "SELECT payload FROM events WHERE session_id = ?1 AND event_type = 'status_change' ORDER BY id",
            )?;
            let statuses = statement
                .query_map([session_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(statuses)
        })
        .expect("read isolated status event stream");
    assert_eq!(status_events.last().map(String::as_str), Some("Starting"));
}

#[test]
fn slow_telemetry_report_only_formats_slow_passes() {
    let report = TelemetryPassTimings {
        total: std::time::Duration::from_millis(750),
        sys_refresh: std::time::Duration::from_millis(25),
        agent_count: 3,
        slow_agents: vec![TelemetrySlowAgent {
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            duration: std::time::Duration::from_millis(620),
        }],
    };

    let message = report.slow_log_message(std::time::Duration::from_millis(500));

    assert!(message.is_some_and(|message| {
        message.contains("total_ms=750")
            && message.contains("agent_count=3")
            && message.contains("agent-1:codex:620ms")
    }));
    assert!(TelemetryPassTimings {
        total: std::time::Duration::from_millis(250),
        sys_refresh: std::time::Duration::from_millis(25),
        agent_count: 1,
        slow_agents: Vec::new(),
    }
    .slow_log_message(std::time::Duration::from_millis(500))
    .is_none());
}

#[test]
fn live_opencode_tui_output_prevents_log_error_from_masking_running_status() {
    let current_status = "Processing...";
    let log_status = "Error".to_string();
    let last_output_at = Some(std::time::SystemTime::now());

    let status = super::reconcile_live_opencode_log_status(
        "opencode",
        current_status,
        log_status,
        Some(true),
        last_output_at,
    );

    assert_eq!(status, current_status);
}

#[test]
fn opencode_log_error_still_applies_without_live_tui_evidence() {
    let status = super::reconcile_live_opencode_log_status(
        "opencode",
        "Processing...",
        "Error".to_string(),
        Some(true),
        None,
    );

    assert_eq!(status, "Error");

    let status = super::reconcile_live_opencode_log_status(
        "opencode",
        "Processing...",
        "Error".to_string(),
        Some(false),
        Some(std::time::SystemTime::now()),
    );

    assert_eq!(status, "Error");
}

#[test]
fn claude_log_status_can_clear_stale_action_needed() {
    let snap = test_snapshot("Action Needed");
    let lines = vec![
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": "Run a tool" }
        }),
        serde_json::json!({
            "type": "system",
            "subtype": "permission_request",
            "tool_name": "Bash"
        }),
        serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "tool-1",
                    "content": "ok"
                }]
            }
        }),
        serde_json::json!({ "type": "system", "subtype": "turn_duration" }),
    ];

    super::apply_claude_log_status(&snap, &lines, false);

    assert_eq!(snap.telemetry_status(), "Idle");
    assert_eq!(*snap.current_status.lock().unwrap(), "Action Needed");
}

#[test]
fn opencode_assistant_text_records_watch_output_and_transcript() {
    let snap = test_snapshot("Processing...");

    super::record_opencode_assistant_text(&snap, "ses_test", "OC_DONE");

    let snapshot = snap
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, Some(4096))
        .unwrap();
    assert!(snapshot.output.text.contains("OC_DONE"));
    assert_eq!(snapshot.transcript.latest_text, "OC_DONE");
    assert_eq!(snapshot.transcript.messages[0].provider, "opencode");
    assert_eq!(
        snapshot.transcript.messages[0].turn_id.as_deref(),
        Some("ses_test")
    );
}

#[test]
fn gemini_assistant_text_records_watch_transcript() {
    let snap = test_snapshot("Processing...");
    let content = concat!(
        r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
        "\n",
        r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
        "\n",
        r#"{"id":"m2","timestamp":"2026-05-14T12:00:03.000Z","type":"model","content":"Gemini answer","tokens":{"input":10,"output":2,"total":12}}"#,
        "\n"
    );

    super::record_latest_gemini_assistant_text(&snap, content);

    let snapshot = snap
        .watch_state
        .lock()
        .unwrap()
        .snapshot_since(None, Some(4096))
        .unwrap();
    assert_eq!(snapshot.transcript.latest_text, "Gemini answer");
    assert_eq!(snapshot.transcript.messages[0].provider, "gemini");
    assert_eq!(
        snapshot.transcript.messages[0].turn_id.as_deref(),
        Some("m2")
    );
}

#[test]
fn gemini_log_matches_legacy_json_session_id() {
    let content = r#"{
          "sessionId": "gemini-session-1",
          "messages": []
        }"#;

    assert!(super::gemini_log_matches_session(
        content,
        "gemini-session-1"
    ));
    assert!(!super::gemini_log_matches_session(content, "other-session"));
}

#[test]
fn gemini_log_matches_jsonl_metadata_session_id() {
    let content = concat!(
        r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
        "\n",
        r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
        "\n"
    );

    assert!(super::gemini_log_matches_session(
        content,
        "gemini-session-1"
    ));
    assert!(!super::gemini_log_matches_session(content, "other-session"));
}

#[test]
fn discover_gemini_log_finds_matching_chat_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    let chats = temp.path().join("project-a").join("chats");
    std::fs::create_dir_all(&chats).expect("chats dir");
    std::fs::write(
        chats.join("other.json"),
        r#"{"sessionId":"other-session","messages":[]}"#,
    )
    .expect("write other chat");
    std::fs::write(
        chats.join("target.json"),
        r#"{"sessionId":"gemini-session-1","messages":[]}"#,
    )
    .expect("write target chat");

    let found = super::discover_gemini_log_in_tmp(temp.path(), "gemini-session-1")
        .expect("matching chat file");
    assert!(found.ends_with("target.json"));
    assert!(super::discover_gemini_log_in_tmp(temp.path(), "missing-session").is_none());
}

#[test]
fn gemini_log_prefix_rejects_id_beyond_prefix_window() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("big.json");
    let mut content = String::from("{\"messages\":[\"");
    content.push_str(&"x".repeat(super::GEMINI_LOG_SESSION_PREFIX_BYTES as usize));
    content.push_str("gemini-session-1\"]}");
    std::fs::write(&path, content).expect("write big chat");

    assert!(!super::gemini_log_prefix_contains(
        &path,
        "gemini-session-1"
    ));
    assert!(!super::gemini_log_prefix_contains(&path, ""));
}

#[test]
fn gemini_log_metrics_parse_legacy_json() {
    let content = r#"{
          "sessionId": "gemini-session-1",
          "startTime": "2026-05-14T12:00:00.000Z",
          "messages": [
            { "type": "user", "timestamp": "2026-05-14T12:00:01.000Z", "content": "hello" },
            { "type": "gemini", "content": "hi" }
          ]
        }"#;

    let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

    assert_eq!(metrics.query_count, 1);
    assert_eq!(
        metrics.init_timestamp.as_deref(),
        Some("2026-05-14T12:00:00.000Z")
    );
    assert_eq!(
        metrics.last_query_timestamp.as_deref(),
        Some("2026-05-14T12:00:01.000Z")
    );
    assert_eq!(metrics.status, Some("Idle"));
}

#[test]
fn pi_log_metrics_parse_latest_user_message_timestamp() {
    let content = concat!(
        r#"{"type":"session","id":"pi-session-1","timestamp":"2026-05-14T12:00:00.000Z"}"#,
        "\n",
        r#"{"type":"message","timestamp":"2026-05-14T12:00:01.000Z","message":{"role":"user","content":"first"}}"#,
        "\n",
        r#"{"type":"message","timestamp":"2026-05-14T12:00:03.000Z","message":{"role":"user","content":"latest"}}"#,
        "\n"
    );

    let metrics = super::parse_pi_log_metrics(content).expect("metrics");

    assert_eq!(metrics.query_count, 2);
    assert_eq!(
        metrics.init_timestamp.as_deref(),
        Some("2026-05-14T12:00:00.000Z")
    );
    assert_eq!(
        metrics.last_query_timestamp.as_deref(),
        Some("2026-05-14T12:00:03.000Z")
    );
}

#[test]
fn gemini_log_metrics_parse_jsonl_completed_message_record() {
    let content = concat!(
        r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
        "\n",
        r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
        "\n",
        r#"{"$set":{"lastUpdated":"2026-05-14T12:00:02.000Z"}}"#,
        "\n",
        r#"{"id":"m2","timestamp":"2026-05-14T12:00:03.000Z","type":"gemini","content":"hi","tokens":{"input":10,"output":1,"total":11}}"#,
        "\n"
    );

    let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

    assert_eq!(metrics.query_count, 1);
    assert_eq!(
        metrics.init_timestamp.as_deref(),
        Some("2026-05-14T12:00:00.000Z")
    );
    assert_eq!(
        metrics.last_query_timestamp.as_deref(),
        Some("2026-05-14T12:00:01.000Z")
    );
    assert_eq!(metrics.status, Some("Idle"));
}

#[test]
fn gemini_log_metrics_jsonl_model_chunk_without_completion_stays_processing() {
    let content = concat!(
        r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
        "\n",
        r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
        "\n",
        r#"{"id":"m2","timestamp":"2026-05-14T12:00:03.000Z","type":"model","content":"partial"}"#,
        "\n"
    );

    let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

    assert_eq!(metrics.query_count, 1);
    assert_eq!(
        metrics.last_query_timestamp.as_deref(),
        Some("2026-05-14T12:00:01.000Z")
    );
    assert_eq!(metrics.status, Some("Processing..."));
}

#[test]
fn gemini_log_metrics_jsonl_result_marks_idle() {
    let content = concat!(
        r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
        "\n",
        r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
        "\n",
        r#"{"id":"m2","timestamp":"2026-05-14T12:00:03.000Z","type":"model","content":"partial"}"#,
        "\n",
        r#"{"type":"result"}"#,
        "\n"
    );

    let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

    assert_eq!(metrics.query_count, 1);
    assert_eq!(metrics.status, Some("Idle"));
}

#[test]
fn gemini_log_metrics_jsonl_last_user_is_processing() {
    let content = concat!(
        r#"{"sessionId":"gemini-session-1","projectHash":"project","startTime":"2026-05-14T12:00:00.000Z"}"#,
        "\n",
        r#"{"id":"m1","timestamp":"2026-05-14T12:00:01.000Z","type":"user","content":"hello"}"#,
        "\n"
    );

    let metrics = super::parse_gemini_log_metrics(content).expect("metrics");

    assert_eq!(metrics.query_count, 1);
    assert_eq!(metrics.status, Some("Processing..."));
}
