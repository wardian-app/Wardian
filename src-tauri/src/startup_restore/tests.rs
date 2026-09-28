use super::*;
use crate::commands::agent::{list_agents, update_agent_config};
use std::future::{poll_fn, Future};
use std::task::Poll;
use tauri::Manager;
use wardian_core::models::{AgentConfig, AgentSessionPersistenceOverride, ProviderConfig};

#[tokio::test]
async fn cancelled_startup_restore_publication_marks_spawn_failed_and_keeps_placeholder() {
    let state = std::sync::Arc::new(AppState::new());
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "claude".into(),
        ..Default::default()
    };
    let publication = RestorePublication::begin(&state, &config.session_id)
        .await
        .expect("restore claim");
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
    let roster_lock = state.agents.lock().await;
    let disposition = crate::manager::SpawnPublicationDisposition::new();
    let failed = disposition.failure_signal();
    let mut spawned = crate::restored_agent_without_process(
        config.clone(),
        "Starting",
        String::new(),
        None,
        None,
    );
    spawned.runtime_generation = Some(7);
    let task_state = state.clone();
    let (ready, received) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        ready.send(()).unwrap();
        publication
            .publish_spawned(&task_state, spawned, disposition)
            .await;
    });
    received.await.expect("restore publication started");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(failed.load(std::sync::atomic::Ordering::Acquire));
    assert!(roster_lock[&config.session_id].runtime_generation.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn restored_codex_keeps_attached_idle_transition_across_placeholder_publication() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "codex".into(),
        ..Default::default()
    };
    let publication = RestorePublication::begin(&state, &config.session_id)
        .await
        .expect("restore claim");
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

    let mut spawned = crate::restored_agent_without_process(
        config.clone(),
        "Starting",
        String::new(),
        Some(4242),
        None,
    );
    spawned.runtime_generation = Some(7);
    spawned
        .watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(true);
    let status = spawned.current_status.clone();

    let roster = state.agents.lock().await;
    assert_eq!(
        crate::manager::codex_onboarding::status_admission(
            app.handle(),
            &config.session_id,
            &status,
            "Idle",
        ),
        crate::manager::codex_onboarding::CodexStatusAdmission::WaitForRoster,
    );
    drop(roster);
    // The deferred transition can wake here while the old placeholder still
    // occupies the roster. Its status-Arc identity check then rejects Idle.
    assert!(
        !crate::manager::codex_onboarding::deferred_status_target_is_ready(
            &state,
            &config.session_id,
            &status,
        )
        .await
    );

    publication
        .publish_spawned(
            &state,
            spawned,
            crate::manager::SpawnPublicationDisposition::new(),
        )
        .await;
    assert_eq!(
        *status.lock().unwrap(),
        "Idle",
        "an attached restored Codex runtime must not remain Starting after publication"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restored_codex_preserves_newer_queued_processing_across_publication() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    for deferred_before_publication in [true, false] {
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let state = app.state::<AppState>();
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            provider: "codex".into(),
            ..Default::default()
        };
        wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
            session_id: &config.session_id,
            session_name: &config.session_id,
            description: "",
            agent_class: "Coder",
            provider: "codex",
            workspace: None,
            project: None,
            is_off: false,
            created_at: None,
        })
        .expect("persist restored agent");
        let publication = RestorePublication::begin(&state, &config.session_id)
            .await
            .expect("restore claim");
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
        let mut spawned = crate::restored_agent_without_process(
            config.clone(),
            "Starting",
            String::new(),
            Some(4242),
            None,
        );
        spawned.runtime_generation = Some(7);
        spawned
            .watch_state
            .lock()
            .unwrap()
            .set_codex_attachment_ready(true);
        let status = spawned.current_status.clone();
        let roster = state.agents.lock().await;
        for next_status in ["Idle", "Processing..."] {
            assert_eq!(
                crate::manager::codex_onboarding::status_admission(
                    app.handle(),
                    &config.session_id,
                    &status,
                    next_status,
                ),
                crate::manager::codex_onboarding::CodexStatusAdmission::WaitForRoster,
            );
            let _current = status.lock().unwrap();
            state.reserve_status_intent(&config.session_id, &status, next_status);
        }
        let processing_revision = state.status_intent_revision(&config.session_id, &status);
        drop(roster);
        if deferred_before_publication {
            assert!(
                crate::manager::codex_onboarding::apply_deferred_status_transition(
                    &state,
                    &config.session_id,
                    &status,
                    "Starting",
                    processing_revision,
                    "Processing...",
                )
                .await
                .is_none(),
                "the placeholder still owns the roster"
            );
        }
        publication
            .publish_spawned(
                &state,
                spawned,
                crate::manager::SpawnPublicationDisposition::new(),
            )
            .await;
        if !deferred_before_publication {
            assert!(
                crate::manager::codex_onboarding::apply_deferred_status_transition(
                    &state,
                    &config.session_id,
                    &status,
                    "Starting",
                    processing_revision,
                    "Processing...",
                )
                .await
                .is_none(),
                "publication must have applied the newer queued status"
            );
        }
        assert_eq!(
            *status.lock().unwrap(),
            "Processing...",
            "deferred_before_publication={deferred_before_publication}"
        );
        wardian_core::db::update_agent_status(&config.session_id, "Processing...", Some(4242))
            .expect("publish restored status");
        assert_eq!(
            wardian_core::db::get_all_agents()
                .expect("read persisted agent")
                .iter()
                .find(|agent| agent.session_id == config.session_id)
                .and_then(|agent| agent.last_status.as_deref()),
            Some("Processing...")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reserved_terminal_status_survives_restored_codex_publication() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    for terminal_status in ["Off", "Error", "Action Needed"] {
        for publication_order in ["before", "waiting", "after"] {
            let app = tauri::test::mock_app();
            app.manage(AppState::new());
            let state = app.state::<AppState>();
            let config = AgentConfig {
                session_id: uuid::Uuid::new_v4().to_string(),
                provider: "codex".into(),
                ..Default::default()
            };
            wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
                session_id: &config.session_id,
                session_name: &config.session_id,
                description: "",
                agent_class: "Coder",
                provider: "codex",
                workspace: None,
                project: None,
                is_off: false,
                created_at: None,
            })
            .expect("persist restored agent");
            let publication = RestorePublication::begin(&state, &config.session_id)
                .await
                .expect("restore claim");
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
            let mut spawned = crate::restored_agent_without_process(
                config.clone(),
                "Starting",
                String::new(),
                Some(4242),
                None,
            );
            spawned.runtime_generation = Some(7);
            spawned
                .watch_state
                .lock()
                .unwrap()
                .set_codex_attachment_ready(true);
            let status = spawned.current_status.clone();

            let roster = if publication_order == "waiting" {
                Some(state.agents.lock().await)
            } else {
                None
            };
            let (started, ready) = tokio::sync::oneshot::channel();
            let task_app = app.handle().clone();
            let published = tokio::spawn(async move {
                started.send(()).unwrap();
                let task_state = task_app.state::<AppState>();
                publication
                    .publish_spawned(
                        &task_state,
                        spawned,
                        crate::manager::SpawnPublicationDisposition::new(),
                    )
                    .await
            });
            if publication_order == "waiting" {
                ready.await.expect("publication entered");
                assert!(!published.is_finished());
            }
            if publication_order == "after" {
                published.await.expect("publication completed");
                assert_eq!(*status.lock().unwrap(), "Idle");
                assert_eq!(
                    crate::manager::codex_onboarding::status_admission(
                        app.handle(),
                        &config.session_id,
                        &status,
                        terminal_status,
                    ),
                    crate::manager::codex_onboarding::CodexStatusAdmission::Allowed,
                );
                let intent_revision = {
                    let _current = status.lock().unwrap();
                    state.reserve_status_intent(&config.session_id, &status, terminal_status)
                };
                assert!(
                    crate::manager::apply_admitted_status_transition(
                        &state,
                        &config.session_id,
                        &status,
                        "Idle",
                        intent_revision,
                        terminal_status,
                    )
                    .is_some(),
                    "the terminal setter wins after publication"
                );
            } else {
                assert_eq!(
                    crate::manager::codex_onboarding::status_admission(
                        app.handle(),
                        &config.session_id,
                        &status,
                        terminal_status,
                    ),
                    crate::manager::codex_onboarding::CodexStatusAdmission::Allowed,
                );
                let intent_revision = {
                    let _current = status.lock().unwrap();
                    state.reserve_status_intent(&config.session_id, &status, terminal_status)
                };
                drop(roster);
                published.await.expect("publication completed");
                assert!(
                    state.status_revision(&config.session_id, &status) > intent_revision,
                    "publication committed the terminal value before the setter resumed"
                );
                assert!(
                    crate::manager::apply_admitted_status_transition(
                        &state,
                        &config.session_id,
                        &status,
                        "Starting",
                        intent_revision,
                        terminal_status,
                    )
                    .is_none(),
                    "publication already committed the terminal intent"
                );
            }
            assert_eq!(
                *status.lock().unwrap(),
                terminal_status,
                "status={terminal_status}, order={publication_order}"
            );
            let observed_at =
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let status_revision = state.status_revision(&config.session_id, &status);
            let status_sequence = state.next_status_observation_sequence(&config.session_id);
            let _lifecycle = state.lock_agent_lifecycle(&config.session_id).await;
            assert_eq!(
                crate::manager::persist_status_observation(
                    &state,
                    &config.session_id,
                    &status,
                    terminal_status,
                    status_sequence,
                    status_revision,
                    &observed_at,
                )
                .await
                .map(|(status, _, _)| status),
                Some(terminal_status.to_string())
            );
            drop(_lifecycle);
            assert_eq!(
                wardian_core::db::get_all_agents()
                    .expect("read persisted agent")
                    .iter()
                    .find(|agent| agent.session_id == config.session_id)
                    .and_then(|agent| agent.last_status.as_deref()),
                Some(terminal_status)
            );
            let agents = state.agents.lock().await;
            let watch = agents[&config.session_id].watch_state.lock().unwrap();
            assert!(
                watch
                    .snapshot_since(None, None)
                    .expect("read status watch")
                    .events
                    .iter()
                    .any(|event| {
                        event.kind == "status"
                            && event.payload["status"]
                                == wardian_core::identity::normalize_status(terminal_status)
                    }),
                "watch observed terminal status={terminal_status}, order={publication_order}"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn queued_processing_while_publication_waits_for_roster_wins() {
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "codex".into(),
        ..Default::default()
    };
    let publication = RestorePublication::begin(&state, &config.session_id)
        .await
        .expect("restore claim");
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
    let mut spawned = crate::restored_agent_without_process(
        config.clone(),
        "Starting",
        String::new(),
        Some(4242),
        None,
    );
    spawned.runtime_generation = Some(7);
    spawned
        .watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(true);
    let status = spawned.current_status.clone();

    let roster = state.agents.lock().await;
    assert_eq!(
        crate::manager::codex_onboarding::status_admission(
            app.handle(),
            &config.session_id,
            &status,
            "Idle",
        ),
        crate::manager::codex_onboarding::CodexStatusAdmission::WaitForRoster,
    );
    let idle_revision = {
        let _current = status.lock().unwrap();
        state.reserve_status_intent(&config.session_id, &status, "Idle")
    };
    let (deferred_started, deferred_ready) = tokio::sync::oneshot::channel();
    let deferred_app = app.handle().clone();
    let deferred_session = config.session_id.clone();
    let deferred_status = status.clone();
    let deferred = tokio::spawn(async move {
        deferred_started.send(()).unwrap();
        let task_state = deferred_app.state::<AppState>();
        crate::manager::codex_onboarding::apply_deferred_status_transition(
            &task_state,
            &deferred_session,
            &deferred_status,
            "Starting",
            idle_revision,
            "Idle",
        )
        .await
    });
    deferred_ready.await.expect("deferred Idle is waiting");

    let (publication_started, publication_ready) = tokio::sync::oneshot::channel();
    let publication_app = app.handle().clone();
    let published = tokio::spawn(async move {
        publication_started.send(()).unwrap();
        let task_state = publication_app.state::<AppState>();
        publication
            .publish_spawned(
                &task_state,
                spawned,
                crate::manager::SpawnPublicationDisposition::new(),
            )
            .await
    });
    publication_ready.await.expect("publication is waiting");
    assert!(!published.is_finished());
    assert_eq!(
        crate::manager::codex_onboarding::status_admission(
            app.handle(),
            &config.session_id,
            &status,
            "Processing...",
        ),
        crate::manager::codex_onboarding::CodexStatusAdmission::WaitForRoster,
    );
    {
        let _current = status.lock().unwrap();
        state.reserve_status_intent(&config.session_id, &status, "Processing...");
    }
    drop(roster);
    assert!(deferred.await.expect("deferred Idle completed").is_none());
    published.await.expect("publication completed");
    assert_eq!(*status.lock().unwrap(), "Processing...");
}

#[tokio::test(flavor = "current_thread")]
async fn queued_restored_codex_idle_does_not_replace_newer_action_needed() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "codex".into(),
        ..Default::default()
    };
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id: &config.session_id,
        session_name: "Restored Codex",
        description: "",
        agent_class: "Coder",
        provider: "codex",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("persist restored agent");
    let publication = RestorePublication::begin(&state, &config.session_id)
        .await
        .expect("restore claim");
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
    let mut spawned = crate::restored_agent_without_process(
        config.clone(),
        "Starting",
        String::new(),
        Some(4242),
        None,
    );
    spawned.runtime_generation = Some(7);
    spawned
        .watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(true);
    let status = spawned.current_status.clone();
    let (expected_status, intent_revision) = {
        let current = status.lock().unwrap();
        (
            current.clone(),
            state.reserve_status_intent(&config.session_id, &status, "Idle"),
        )
    };
    let roster = state.agents.lock().await;
    assert_eq!(
        crate::manager::codex_onboarding::status_admission(
            app.handle(),
            &config.session_id,
            &status,
            "Idle",
        ),
        crate::manager::codex_onboarding::CodexStatusAdmission::WaitForRoster,
    );
    drop(roster);

    // Keep the queued work pending until after publication so it observes the
    // same runtime Arc with a newer approval status.
    let (release, pending) = tokio::sync::oneshot::channel::<()>();
    let task_app = app.handle().clone();
    let task_session_id = config.session_id.clone();
    let task_status = status.clone();
    let queued = tokio::spawn(async move {
        pending.await.expect("release queued transition");
        let task_state = task_app.state::<AppState>();
        crate::manager::codex_onboarding::apply_deferred_status_transition(
            &task_state,
            &task_session_id,
            &task_status,
            &expected_status,
            intent_revision,
            "Idle",
        )
        .await
    });
    {
        let mut current = status.lock().unwrap();
        *current = "Action Needed".to_string();
        state.commit_status_revision(&config.session_id, &status, "Action Needed");
    }
    wardian_core::db::update_agent_status(&config.session_id, "Action Needed", Some(4242))
        .expect("persist approval status");
    publication
        .publish_spawned(
            &state,
            spawned,
            crate::manager::SpawnPublicationDisposition::new(),
        )
        .await;
    release.send(()).expect("run queued transition");
    let applied = queued.await.expect("queued transition completes");
    if applied.is_some() {
        wardian_core::db::update_agent_status(&config.session_id, "Idle", Some(4242))
            .expect("persist accepted deferred transition");
    }
    assert!(
        applied.is_none(),
        "superseded Idle must not schedule persistence"
    );
    assert_eq!(*status.lock().unwrap(), "Action Needed");
    assert_eq!(
        state.agents.lock().await[&config.session_id]
            .current_status
            .lock()
            .unwrap()
            .as_str(),
        "Action Needed"
    );
    let persisted = wardian_core::db::get_all_agents().expect("read persisted agent");
    assert_eq!(
        persisted
            .iter()
            .find(|agent| agent.session_id == config.session_id)
            .and_then(|agent| agent.last_status.as_deref()),
        Some("Action Needed")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn queued_idle_applies_when_attached_runtime_status_is_unchanged() {
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "codex".into(),
        ..Default::default()
    };
    let mut agent = crate::restored_agent_without_process(
        config.clone(),
        "Starting",
        String::new(),
        Some(4242),
        None,
    );
    agent.runtime_generation = Some(7);
    agent
        .watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(true);
    let watch_state = agent.watch_state.clone();
    let status = agent.current_status.clone();
    state
        .agents
        .lock()
        .await
        .insert(config.session_id.clone(), agent);

    let roster = state.agents.lock().await;
    assert_eq!(
        crate::manager::codex_onboarding::status_admission(
            app.handle(),
            &config.session_id,
            &status,
            "Idle",
        ),
        crate::manager::codex_onboarding::CodexStatusAdmission::WaitForRoster,
    );
    let intent_revision = {
        let _current = status.lock().unwrap();
        state.reserve_status_intent(&config.session_id, &status, "Idle")
    };
    drop(roster);
    watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(false);
    assert_eq!(
        crate::manager::codex_onboarding::status_admission(
            app.handle(),
            &config.session_id,
            &status,
            "Processing...",
        ),
        crate::manager::codex_onboarding::CodexStatusAdmission::Blocked,
    );
    assert_eq!(
        state.status_intent_revision(&config.session_id, &status),
        intent_revision
    );
    watch_state.lock().unwrap().set_codex_attachment_ready(true);
    assert!(
        crate::manager::codex_onboarding::apply_deferred_status_transition(
            &state,
            &config.session_id,
            &status,
            "Starting",
            intent_revision,
            "Idle",
        )
        .await
        .is_some()
    );
    assert_eq!(*status.lock().unwrap(), "Idle");
}

#[tokio::test(flavor = "current_thread")]
async fn newer_queued_idle_wins_after_queued_processing_under_roster_contention() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "codex".into(),
        ..Default::default()
    };
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id: &config.session_id,
        session_name: "Queued Codex",
        description: "",
        agent_class: "Coder",
        provider: "codex",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("persist queued agent");
    let mut agent = crate::restored_agent_without_process(
        config.clone(),
        "Starting",
        String::new(),
        Some(4242),
        None,
    );
    agent.runtime_generation = Some(7);
    agent
        .watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(true);
    let status = agent.current_status.clone();
    state
        .agents
        .lock()
        .await
        .insert(config.session_id.clone(), agent);

    let roster = state.agents.lock().await;
    let mut revisions = Vec::new();
    for next_status in ["Processing...", "Idle"] {
        let _current = status.lock().unwrap();
        revisions.push(state.reserve_status_intent(&config.session_id, &status, next_status));
        assert_eq!(
            crate::manager::codex_onboarding::status_admission(
                app.handle(),
                &config.session_id,
                &status,
                next_status,
            ),
            crate::manager::codex_onboarding::CodexStatusAdmission::WaitForRoster,
        );
    }
    drop(roster);
    let older_applied = crate::manager::codex_onboarding::apply_deferred_status_transition(
        &state,
        &config.session_id,
        &status,
        "Starting",
        revisions[0],
        "Processing...",
    )
    .await;
    assert!(
        older_applied.is_none(),
        "older queued Processing must be superseded"
    );
    assert!(
        crate::manager::codex_onboarding::apply_deferred_status_transition(
            &state,
            &config.session_id,
            &status,
            "Starting",
            revisions[1],
            "Idle",
        )
        .await
        .is_some(),
        "the later queued Idle must remain applicable after Processing"
    );
    assert_eq!(*status.lock().unwrap(), "Idle");
    wardian_core::db::update_agent_status(&config.session_id, "Idle", Some(4242))
        .expect("persist accepted deferred Idle");
    assert_eq!(
        wardian_core::db::get_all_agents()
            .expect("read persisted agent")
            .iter()
            .find(|agent| agent.session_id == config.session_id)
            .and_then(|agent| agent.last_status.as_deref()),
        Some("Idle")
    );
}

#[tokio::test]
async fn old_runtime_status_intent_cannot_cancel_provisional_replacement_intent() {
    let state = AppState::new();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "codex".into(),
        ..Default::default()
    };
    let placeholder = crate::restored_agent_without_process(
        config.clone(),
        "Restoring",
        String::new(),
        None,
        None,
    );
    let old_status = placeholder.current_status.clone();
    state
        .agents
        .lock()
        .await
        .insert(config.session_id.clone(), placeholder);
    let mut replacement = crate::restored_agent_without_process(
        config.clone(),
        "Starting",
        String::new(),
        Some(4242),
        None,
    );
    replacement.runtime_generation = Some(7);
    replacement
        .watch_state
        .lock()
        .unwrap()
        .set_codex_attachment_ready(true);
    let new_status = replacement.current_status.clone();
    // The new owner can report a status before replacing the old roster entry.
    let new_revision = {
        let _current = new_status.lock().unwrap();
        state.reserve_status_intent(&config.session_id, &new_status, "Idle")
    };
    state
        .agents
        .lock()
        .await
        .insert(config.session_id.clone(), replacement);
    let old_revision = {
        let _current = old_status.lock().unwrap();
        state.reserve_status_intent(&config.session_id, &old_status, "Processing...")
    };
    assert!(old_revision > new_revision);
    assert_eq!(
        state.status_intent_revision(&config.session_id, &new_status),
        new_revision
    );
    assert!(
        crate::manager::codex_onboarding::apply_deferred_status_transition(
            &state,
            &config.session_id,
            &new_status,
            "Starting",
            new_revision,
            "Idle",
        )
        .await
        .is_some(),
        "a stale owner's intent cannot invalidate the new owner's queued status"
    );
    assert_eq!(*new_status.lock().unwrap(), "Idle");
}

#[test]
fn no_op_idle_intent_preserves_the_committed_status_revision() {
    let state = AppState::new();
    let status = Arc::new(Mutex::new("Idle".to_string()));
    let committed_revision = {
        let _current = status.lock().unwrap();
        state.commit_status_revision("agent", &status, "Idle")
    };
    let intent_revision = {
        let _current = status.lock().unwrap();
        state.reserve_status_intent("agent", &status, "Idle")
    };
    assert!(intent_revision > committed_revision);
    assert_eq!(state.status_revision("agent", &status), committed_revision);
    assert_eq!(
        state.status_intent_revision("agent", &status),
        intent_revision
    );
    assert_eq!(
        state.status_intent_status("agent", &status).as_deref(),
        Some("Idle")
    );
}

#[tokio::test]
async fn restored_status_handoff_preserves_unattached_stale_and_other_provider_states() {
    for (provider, attachment_ready, runtime_generation, initial_status) in [
        ("codex", false, Some(7), "Starting"),
        ("codex", true, None, "Starting"),
        ("claude", true, Some(7), "Starting"),
        ("codex", true, Some(7), "Action Needed"),
        ("codex", true, Some(7), "Off"),
    ] {
        let state = AppState::new();
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            provider: provider.into(),
            ..Default::default()
        };
        let publication = RestorePublication::begin(&state, &config.session_id)
            .await
            .expect("restore claim");
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
        let mut spawned = crate::restored_agent_without_process(
            config.clone(),
            initial_status,
            String::new(),
            None,
            None,
        );
        spawned.runtime_generation = runtime_generation;
        spawned
            .watch_state
            .lock()
            .unwrap()
            .set_codex_attachment_ready(attachment_ready);
        let status = spawned.current_status.clone();
        publication
            .publish_spawned(
                &state,
                spawned,
                crate::manager::SpawnPublicationDisposition::new(),
            )
            .await;
        assert_eq!(
            *status.lock().unwrap(),
            initial_status,
            "provider={provider}, attached={attachment_ready}, generation={runtime_generation:?}"
        );
    }
}

#[tokio::test]
async fn failed_restore_exposes_provider_error_to_late_terminal_presentations() {
    use crate::state::terminal_session::{TerminalClientIdentity, TerminalRuntimeHandles};
    use wardian_core::models::*;

    for provider in ["codex", "claude"] {
        let state = AppState::new();
        let session_id = format!("failed-{provider}");
        let config = AgentConfig {
            session_id: session_id.clone(),
            provider: provider.into(),
            ..Default::default()
        };
        let publication = RestorePublication::begin(&state, &session_id)
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
        let error =
            format!("Wardian could not restore this agent.\r\n{provider}: launch failed\r\n");
        publication
            .publish(
                &state,
                crate::restored_agent_without_process(config, "Error", error, None, None),
            )
            .await;

        let broker = &state.terminal_sessions;
        let failed = broker.broker_state(&session_id).await.unwrap();
        assert_eq!(failed.runtime_state, TerminalRuntimeState::Paused);
        assert!(
            state.agents.lock().await[&session_id]
                .runtime_generation
                .is_none(),
            "a diagnostic presentation must not impersonate a native child"
        );
        for presentation_id in ["first-view", "reopened-view"] {
            broker
                .register_presentation(
                    TerminalPresentationRegistration {
                        presentation_id: presentation_id.into(),
                        session_id: session_id.clone(),
                        client_kind: TerminalClientKind::Desktop,
                        desired_geometry: None,
                        visibility: TerminalVisibility::Visible,
                        render_state: TerminalRenderState::Mounted,
                        requested_interaction: TerminalRequestedInteraction::Interactive,
                        observed_lease_epoch: failed.lease_epoch,
                    },
                    TerminalClientIdentity::trusted_desktop(),
                )
                .await
                .unwrap();
            let snapshot = broker.snapshot(&session_id).await.unwrap();
            assert!(snapshot
                .visible_grid
                .contains(&format!("{provider}: launch failed")));
            let activation = broker
                .begin_activation(TerminalActivationBeginRequest {
                    session_id: session_id.clone(),
                    presentation_id: presentation_id.into(),
                    runtime_generation: failed.runtime_generation,
                    observed_lease_epoch: failed.lease_epoch,
                })
                .await
                .unwrap();
            assert_eq!(
                activation.decision.status,
                TerminalLeaseDecisionStatus::Rejected
            );
            broker
                .unregister_presentation(&session_id, presentation_id, failed.runtime_generation)
                .await
                .unwrap();
        }

        let (input, _receiver) = tokio::sync::mpsc::channel(1);
        let replacement = broker
            .start_or_replace_runtime(
                &session_id,
                TerminalRuntimeHandles::new(input, |_| Ok(())),
                TerminalGeometry { cols: 80, rows: 24 },
            )
            .await
            .unwrap();
        assert!(replacement > failed.runtime_generation);
        assert_eq!(
            broker
                .broker_state(&session_id)
                .await
                .unwrap()
                .runtime_state,
            TerminalRuntimeState::Live
        );
        assert!(!broker
            .snapshot(&session_id)
            .await
            .unwrap()
            .visible_grid
            .contains("launch failed"));
        broker
            .terminate_and_remove_runtime(&session_id, replacement)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn failed_restore_terminal_cleanup_needs_no_native_generation() {
    let state = AppState::new();
    let session_id = uuid::Uuid::new_v4().to_string();
    let config = AgentConfig {
        session_id: session_id.clone(),
        session_name: "Failed restore".into(),
        provider: "claude".into(),
        ..Default::default()
    };
    let publication = RestorePublication::begin(&state, &session_id)
        .await
        .unwrap();
    publication
        .publish(
            &state,
            crate::restored_agent_without_process(
                config,
                "Error",
                "Provider launch failed\r\n".into(),
                None,
                None,
            ),
        )
        .await;
    assert!(state.terminal_sessions.snapshot(&session_id).await.is_ok());

    state
        .terminal_sessions
        .remove_agent_session(&session_id, None)
        .await
        .unwrap();

    assert_eq!(
        state.terminal_sessions.snapshot(&session_id).await,
        Err(crate::state::terminal_session::TerminalBrokerError::SessionNotFound)
    );
}

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

async fn publish_error_placeholder(
    state: &AppState,
    config: &AgentConfig,
) -> std::sync::Arc<std::sync::Mutex<String>> {
    let publication = RestorePublication::begin(state, &config.session_id)
        .await
        .expect("initial restore claim");
    let status = publication
        .publish(
            state,
            crate::restored_agent_without_process(
                config.clone(),
                "Error",
                "startup restore was withheld".into(),
                None,
                None,
            ),
        )
        .await;
    drop(publication);
    status
}

#[test]
fn restore_retry_is_scheduled_only_for_the_reported_active_lifecycle_lease() {
    let config = AgentConfig {
        session_id: "agent-1".into(),
        provider: "codex".into(),
        resume_session: Some("resume-1".into()),
        ..Default::default()
    };
    let expiry = chrono::DateTime::parse_from_rfc3339("2026-09-27T12:05:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: config.session_id.clone(),
        provider: config.provider.clone(),
        resume_session: "resume-1".into(),
        owner_kind: "agent_lifecycle".into(),
        owner_id: "resume:operation".into(),
        acquisition_id: "acquisition-1".into(),
        owner_node_id: None,
        mode: "lifecycle_transition".into(),
        started_at: "2026-09-27T11:45:00Z".into(),
        heartbeat_at: "2026-09-27T11:55:00Z".into(),
        expires_at: expiry.to_rfc3339(),
    };
    let error = format!(
        "provider startup was withheld because conversation {} is leased by {} {} ({})",
        config.session_id, lease.owner_kind, lease.owner_id, lease.mode
    );

    assert_eq!(
        super::retryable_lifecycle_restore_lease(&config, &error, std::slice::from_ref(&lease)),
        Some(lease.clone())
    );
    assert!(super::retryable_lifecycle_restore_lease(
        &config,
        "provider startup failed for another reason",
        std::slice::from_ref(&lease),
    )
    .is_none());

    let mut background = lease.clone();
    background.mode = "background_resume".into();
    assert!(super::retryable_lifecycle_restore_lease(&config, &error, &[background]).is_none());

    let mut expired = lease.clone();
    expired.expires_at = "2026-09-27T11:59:59Z".into();
    assert_eq!(
        super::retryable_lifecycle_restore_lease(&config, &error, &[expired.clone()]),
        Some(expired),
        "an expiry race still schedules a retry through the normal gates"
    );

    let mut provider_spawn = lease.clone();
    provider_spawn.owner_kind = "provider_spawn".into();
    provider_spawn.owner_id = "81884:restore-attempt".into();
    let provider_spawn_error = format!(
        "provider startup was withheld because conversation {} is leased by {} {} ({})",
        config.session_id, provider_spawn.owner_kind, provider_spawn.owner_id, provider_spawn.mode
    );
    assert_eq!(
        super::retryable_lifecycle_restore_lease(
            &config,
            &provider_spawn_error,
            std::slice::from_ref(&provider_spawn),
        ),
        Some(provider_spawn)
    );

    let mut pause = lease.clone();
    pause.owner_id = "pause:operation".into();
    let pause_error = format!(
        "provider startup was withheld because conversation {} is leased by {} {} ({})",
        config.session_id, pause.owner_kind, pause.owner_id, pause.mode
    );
    assert!(
        super::retryable_lifecycle_restore_lease(&config, &pause_error, &[pause]).is_none(),
        "a pause lease must never trigger an automatic restore"
    );

    let mut uncertain = wardian_core::conversation_lease::ConversationLease {
        owner_kind: "prior_provider_hold".into(),
        mode: "uncertain_previous_provider".into(),
        ..lease
    };
    uncertain.expires_at = "9999-12-31T23:59:59Z".into();
    let uncertain_error = format!(
        "provider startup was withheld because conversation {} is leased by {} {} ({})",
        config.session_id, uncertain.owner_kind, uncertain.owner_id, uncertain.mode
    );
    assert!(
        super::retryable_lifecycle_restore_lease(&config, &uncertain_error, &[uncertain]).is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn restore_retry_runner_waits_without_a_gate_then_calls_the_normal_spawn_gate_once() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let state = std::sync::Arc::new(AppState::new());
    let config = AgentConfig {
        session_id: "retry-after-expiry".into(),
        provider: "codex".into(),
        resume_session: Some("resume-1".into()),
        ..Default::default()
    };
    let expected_status = publish_error_placeholder(&state, &config).await;
    let task_state = state.clone();
    let task_config = config.clone();
    let expected_config_for_task = task_config.clone();
    let task_status = expected_status.clone();
    let state_for_attempt = state.clone();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts_for_task = attempts.clone();
    let (release_wait, wait_released) = tokio::sync::oneshot::channel();
    let retry = tokio::spawn(async move {
        super::retry_once_after_lifecycle_lease_clear(
            &task_state,
            &expected_config_for_task,
            &task_status,
            move || async move { wait_released.await.is_ok() },
            || async { Some(()) },
            move |publication, ()| async move {
                assert!(
                    state_for_attempt
                        .try_lock_agent_lifecycle(&task_config.session_id)
                        .await
                        .is_none(),
                    "the retry attempt must hold the lifecycle gate"
                );
                attempts_for_task.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                let lease =
                    crate::manager::spawn::provider_spawn_lease_for_launch(&task_config, None)
                        .expect("the normal provider spawn lease is still used");
                drop(lease);
                drop(publication);
            },
        )
        .await
    });

    tokio::task::yield_now().await;
    let lifecycle_probe = state.try_lock_agent_lifecycle(&config.session_id).await;
    assert!(
        lifecycle_probe.is_some(),
        "the expiry wait must not hold the gate"
    );
    drop(lifecycle_probe);

    assert!(!retry.is_finished(), "the retry must await lease clearance");
    release_wait.send(()).expect("release retry wait");
    retry.await.unwrap().expect("one retried spawn attempt");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[tokio::test(start_paused = true)]
async fn restore_retry_wait_tracks_renewed_lease_until_the_exact_owner_releases_it() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = AgentConfig {
        session_id: "retry-renewed-lease".into(),
        provider: "codex".into(),
        resume_session: Some("resume-renewed".into()),
        ..Default::default()
    };
    let base = chrono::DateTime::parse_from_rfc3339("2026-09-27T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let started_at = base.to_rfc3339();
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: config.session_id.clone(),
        provider: config.provider.clone(),
        resume_session: "resume-renewed".into(),
        owner_kind: "agent_lifecycle".into(),
        owner_id: "resume:long-operation".into(),
        acquisition_id: "same-acquisition".into(),
        owner_node_id: None,
        mode: "lifecycle_transition".into(),
        started_at: started_at.clone(),
        heartbeat_at: started_at.clone(),
        expires_at: (base + chrono::Duration::seconds(4)).to_rfc3339(),
    };
    let leases = std::sync::Arc::new(std::sync::Mutex::new(vec![lease.clone()]));
    let start = tokio::time::Instant::now();
    let state = std::sync::Arc::new(AppState::new());
    let expected_status = publish_error_placeholder(&state, &config).await;
    let task_state = state.clone();
    let expected_config = config.clone();
    let state_for_attempt = state.clone();
    let session_id_for_attempt = config.session_id.clone();
    let task_status = expected_status.clone();
    let task_config = config.clone();
    let expected_lease = lease.clone();
    let leases_for_task = leases.clone();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts_for_task = attempts.clone();
    let retry = tokio::spawn(async move {
        super::retry_once_after_lifecycle_lease_clear(
            &task_state,
            &expected_config,
            &task_status,
            move || async move {
                super::wait_for_lifecycle_restore_lease(
                    &task_config,
                    &expected_lease,
                    move || Ok(leases_for_task.lock().unwrap().clone()),
                    move || {
                        base + chrono::Duration::from_std(start.elapsed())
                            .expect("tokio elapsed duration is representable")
                    },
                )
                .await
                .expect("lease store remains readable")
            },
            || async { Some(()) },
            move |publication, ()| async move {
                assert!(
                    state_for_attempt
                        .try_lock_agent_lifecycle(&session_id_for_attempt)
                        .await
                        .is_none(),
                    "retry must acquire the lifecycle gate after the lease wait"
                );
                attempts_for_task.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                drop(publication);
            },
        )
        .await
    });

    tokio::task::yield_now().await;
    let lifecycle_probe = state.try_lock_agent_lifecycle(&config.session_id).await;
    assert!(
        lifecycle_probe.is_some(),
        "the lease wait must not hold the gate"
    );
    drop(lifecycle_probe);
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    {
        let mut current = leases.lock().unwrap();
        current[0].heartbeat_at = (base + chrono::Duration::seconds(2)).to_rfc3339();
        current[0].expires_at = (base + chrono::Duration::seconds(12)).to_rfc3339();
    }
    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    tokio::task::yield_now().await;
    assert!(
        !retry.is_finished(),
        "a renewed exact lease must extend the wait past its original expiry"
    );
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Acquire), 0);
    leases.lock().unwrap().clear();
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    retry.await.unwrap().expect("retry after owner release");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[tokio::test(start_paused = true)]
async fn restore_retry_cancels_after_a_config_change_during_the_wait() {
    let state = std::sync::Arc::new(AppState::new());
    let config = AgentConfig {
        session_id: "retry-config-changed".into(),
        ..Default::default()
    };
    let expected_status = publish_error_placeholder(&state, &config).await;
    let task_state = state.clone();
    let task_config = config.clone();
    let task_status = expected_status;
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts_for_task = attempts.clone();
    let (release_wait, wait_released) = tokio::sync::oneshot::channel();
    let retry = tokio::spawn(async move {
        super::retry_once_after_lifecycle_lease_clear(
            &task_state,
            &task_config,
            &task_status,
            move || async move { wait_released.await.is_ok() },
            || async { Some(()) },
            move |publication, ()| async move {
                attempts_for_task.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                drop(publication);
            },
        )
        .await
    });

    let _lifecycle = state.lock_agent_lifecycle(&config.session_id).await;
    {
        let agents = state.agents.lock().await;
        agents[&config.session_id]
            .config
            .lock()
            .unwrap()
            .description = "user edit".into();
    }
    drop(_lifecycle);
    release_wait.send(()).expect("release retry wait");

    assert!(retry.await.unwrap().is_none());
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Acquire), 0);
    assert_eq!(
        state.agents.lock().await[&config.session_id]
            .config
            .lock()
            .unwrap()
            .description,
        "user edit"
    );
}

#[tokio::test(start_paused = true)]
async fn restore_retry_cancels_after_a_user_operation_replaces_error_status() {
    let state = std::sync::Arc::new(AppState::new());
    let config = AgentConfig {
        session_id: "retry-user-operated".into(),
        ..Default::default()
    };
    let expected_status = publish_error_placeholder(&state, &config).await;
    let task_state = state.clone();
    let task_config = config.clone();
    let task_status = expected_status;
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts_for_task = attempts.clone();
    let (release_wait, wait_released) = tokio::sync::oneshot::channel();
    let retry = tokio::spawn(async move {
        super::retry_once_after_lifecycle_lease_clear(
            &task_state,
            &task_config,
            &task_status,
            move || async move { wait_released.await.is_ok() },
            || async { Some(()) },
            move |publication, ()| async move {
                attempts_for_task.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                drop(publication);
            },
        )
        .await
    });

    let _lifecycle = state.lock_agent_lifecycle(&config.session_id).await;
    *state.agents.lock().await[&config.session_id]
        .current_status
        .lock()
        .unwrap() = "Processing...".into();
    drop(_lifecycle);
    release_wait.send(()).expect("release retry wait");

    assert!(retry.await.unwrap().is_none());
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Acquire), 0);
}

#[tokio::test]
async fn restore_retry_hands_its_exact_provider_spawn_lease_to_launch() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "mock".into(),
        provider_config: ProviderConfig::Mock(Default::default()),
        resume_session: Some(uuid::Uuid::new_v4().to_string()),
        ..Default::default()
    };
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&config))
        .expect("persist selected config");
    let saved: Vec<AgentConfig> = serde_json::from_slice(
        &std::fs::read(_home.directory.path().join("settings/state.json"))
            .expect("read selected config"),
    )
    .expect("decode selected config");
    assert_eq!(
        serde_json::to_value(&saved).unwrap(),
        serde_json::to_value(std::slice::from_ref(&config)).unwrap(),
        "retry requires the saved provider config to match the selected snapshot"
    );
    let now = chrono::Utc::now();
    let previous = wardian_core::conversation_lease::ConversationLease {
        agent_id: config.session_id.clone(),
        provider: config.provider.clone(),
        resume_session: config.resume_session.clone().unwrap(),
        owner_kind: "agent_lifecycle".into(),
        owner_id: uuid::Uuid::new_v4().to_string(),
        acquisition_id: uuid::Uuid::new_v4().to_string(),
        owner_node_id: None,
        mode: "lifecycle_transition".into(),
        started_at: now.to_rfc3339(),
        heartbeat_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    assert!(matches!(
        wardian_core::conversation_lease::try_acquire_lease(previous.clone(), &now.to_rfc3339())
            .expect("persist previous lifecycle owner"),
        wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Acquired
    ));
    assert!(
        crate::manager::spawn::provider_spawn_lease_for_launch(&config, None).is_err(),
        "the first restore must wait for the previous lifecycle owner"
    );

    let state = AppState::new();
    let expected_status = publish_error_placeholder(&state, &config).await;
    let previous_owner = previous.owner();
    let retry_config = config.clone();
    let result = super::retry_once_after_lifecycle_lease_clear(
        &state,
        &config,
        &expected_status,
        move || async move {
            wardian_core::conversation_lease::release_lease_owner_persisted(&previous_owner)
                .expect("previous owner releases its exact lease");
            true
        },
        || async { Some(()) },
        move |publication, ()| async move {
            let lease = crate::manager::spawn::acquire_restore_retry_spawn_lease(
                std::slice::from_ref(&retry_config),
                &retry_config,
            )?
            .expect("saved config still permits retry");
            assert_eq!(lease.owner().owner_kind, "provider_spawn");
            let owner = lease.owner().clone();
            let mut inherited =
                crate::manager::spawn::provider_spawn_lease_for_launch(&retry_config, Some(lease))?;
            assert_eq!(inherited.owner(), &owner);
            assert!(wardian_core::conversation_lease::load_leases_checked()?
                .iter()
                .any(|entry| entry.owner() == owner && entry.mode == "lifecycle_transition"));
            inherited.release()?;
            drop(publication);
            Ok::<(), String>(())
        },
    )
    .await
    .expect("retry reclaim succeeds");
    assert!(result.is_ok(), "retry lease handoff failed: {result:?}");
    assert!(wardian_core::conversation_lease::load_leases_checked()
        .expect("read released retry lease")
        .is_empty());
}

#[tokio::test]
async fn restore_retry_does_not_resurrect_a_cross_process_removed_agent() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "mock".into(),
        ..Default::default()
    };
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&config))
        .expect("persist selected config");
    let state = std::sync::Arc::new(AppState::new());
    let expected_status = publish_error_placeholder(&state, &config).await;
    let task_state = state.clone();
    let task_config = config.clone();
    let check_config = task_config.clone();
    let task_status = expected_status;
    let (release_wait, wait_released) = tokio::sync::oneshot::channel();
    let retry = tokio::spawn(async move {
        super::retry_once_after_lifecycle_lease_clear(
            &task_state,
            &task_config,
            &task_status,
            move || async move { wait_released.await.is_ok() },
            || async { Some(()) },
            move |publication, ()| async move {
                let lease = crate::manager::spawn::acquire_restore_retry_spawn_lease(
                    std::slice::from_ref(&check_config),
                    &check_config,
                )
                .expect("roster and provider lease check succeeds");
                drop(publication);
                lease.is_some()
            },
        )
        .await
    });

    crate::manager::try_save_state_snapshot(&[]).expect("other process removes agent");
    release_wait.send(()).expect("release retry wait");
    assert_eq!(retry.await.unwrap(), Some(false));
    assert!(wardian_core::conversation_lease::load_leases_checked()
        .expect("read conversation ownership")
        .is_empty());
    assert!(std::fs::read_to_string(
        crate::manager::get_wardian_home()
            .unwrap()
            .join("settings")
            .join("state.json")
    )
    .expect("read durable roster")
    .contains("[]"));
}

#[tokio::test]
async fn restore_retry_does_not_override_a_cross_process_config_edit_or_pause() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "claude".into(),
        ..Default::default()
    };
    let changed = AgentConfig {
        description: "edited in another Wardian process".into(),
        ..config.clone()
    };
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&config))
        .expect("persist selected config");
    let state = std::sync::Arc::new(AppState::new());
    let expected_status = publish_error_placeholder(&state, &config).await;
    let task_state = state.clone();
    let task_config = config.clone();
    let check_config = task_config.clone();
    let task_status = expected_status;
    let (release_wait, wait_released) = tokio::sync::oneshot::channel();
    let retry = tokio::spawn(async move {
        super::retry_once_after_lifecycle_lease_clear(
            &task_state,
            &task_config,
            &task_status,
            move || async move { wait_released.await.is_ok() },
            || async { Some(()) },
            move |publication, ()| async move {
                let lease = crate::manager::spawn::acquire_restore_retry_spawn_lease(
                    std::slice::from_ref(&check_config),
                    &check_config,
                )
                .expect("roster and provider lease check succeeds");
                drop(publication);
                lease.is_some()
            },
        )
        .await
    });

    crate::manager::try_save_state_snapshot(std::slice::from_ref(&changed))
        .expect("other process edits the durable config");
    release_wait.send(()).expect("release retry wait");
    assert_eq!(retry.await.unwrap(), Some(false));
    let leases = wardian_core::conversation_lease::load_leases_checked()
        .expect("read conversation ownership");
    assert!(leases.is_empty());
    let saved: Vec<AgentConfig> = serde_json::from_str(
        &std::fs::read_to_string(
            crate::manager::get_wardian_home()
                .unwrap()
                .join("settings")
                .join("state.json"),
        )
        .expect("read durable roster"),
    )
    .expect("decode durable roster");
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(vec![changed]).unwrap()
    );

    let paused = AgentConfig {
        is_off: true,
        ..config.clone()
    };
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&paused))
        .expect("other process pauses agent");
    let no_spawn_lease = crate::manager::spawn::acquire_restore_retry_spawn_lease(
        std::slice::from_ref(&config),
        &config,
    )
    .expect("read paused config");
    assert!(no_spawn_lease.is_none());
    assert!(wardian_core::conversation_lease::load_leases_checked()
        .expect("read conversation ownership")
        .is_empty());
}

#[tokio::test]
async fn successful_restore_persists_runtime_config_without_overwriting_a_later_edit() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let selected_config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "claude".into(),
        ..Default::default()
    };
    let restored_config = AgentConfig {
        resume_session: Some("prepared-provider-session".into()),
        ..selected_config.clone()
    };
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&selected_config))
        .expect("persist selected config");

    assert!(
        crate::manager::spawn::persist_restore_retry_config_if_unchanged(
            std::slice::from_ref(&selected_config),
            &restored_config,
        )
        .expect("persist successful restore config")
    );
    let state_path = crate::manager::get_wardian_home()
        .unwrap()
        .join("settings")
        .join("state.json");
    let saved: Vec<AgentConfig> =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("read state"))
            .expect("decode state");
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(vec![restored_config.clone()]).unwrap()
    );

    let later_edit = AgentConfig {
        description: "saved after the retry acquired its lease".into(),
        ..restored_config
    };
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&later_edit))
        .expect("persist later edit");
    let stale_retry_config = AgentConfig {
        session_name: "stale retry".into(),
        ..selected_config.clone()
    };
    assert!(
        !crate::manager::spawn::persist_restore_retry_config_if_unchanged(
            std::slice::from_ref(&selected_config),
            &stale_retry_config,
        )
        .expect("preserve later edit")
    );
    let saved_after_edit: Vec<AgentConfig> =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("read state"))
            .expect("decode state");
    assert_eq!(
        serde_json::to_value(saved_after_edit).unwrap(),
        serde_json::to_value(vec![later_edit]).unwrap()
    );
}

#[tokio::test]
async fn startup_roster_persistence_preserves_cross_process_pause_and_removal() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let _home = TestHome::new();
    let selected_config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "claude".into(),
        ..Default::default()
    };
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&selected_config))
        .expect("persist selected config");
    let state = AppState::new();
    let publication = RestorePublication::begin(&state, &selected_config.session_id)
        .await
        .expect("startup restore claim");
    publication
        .publish(
            &state,
            crate::restored_agent_without_process(
                selected_config.clone(),
                "Error",
                "provider restore was withheld".into(),
                None,
                None,
            ),
        )
        .await;
    drop(publication);

    let paused_config = AgentConfig {
        is_off: true,
        description: "paused in another Wardian process".into(),
        ..selected_config.clone()
    };
    crate::manager::try_save_state_snapshot(std::slice::from_ref(&paused_config))
        .expect("other process pauses and edits the agent");
    super::persist_roster(&state, std::slice::from_ref(&selected_config))
        .await
        .expect("persist startup roster without replacing pause");
    let state_path = crate::manager::get_wardian_home()
        .unwrap()
        .join("settings")
        .join("state.json");
    let saved: Vec<AgentConfig> =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("read state"))
            .expect("decode state");
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(vec![paused_config]).unwrap()
    );

    crate::manager::try_save_state_snapshot(&[]).expect("other process removes the agent");
    super::persist_roster(&state, std::slice::from_ref(&selected_config))
        .await
        .expect("persist startup roster without recreating removed entry");
    let saved: Vec<AgentConfig> =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("read state"))
            .expect("decode state");
    assert!(saved.is_empty());
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
        };
        persist_roster(&state, std::slice::from_ref(&config))
            .await
            .unwrap();
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
    persist_roster(&state, std::slice::from_ref(&config))
        .await
        .unwrap();
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
    let mut persist = std::pin::pin!(persist_roster(&state, &[]));
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
    state
        .agent_order
        .lock()
        .await
        .push(config.session_id.clone());
    crate::manager::try_save_state_snapshot_unlocked(std::slice::from_ref(&config))
        .expect("competing writer persists its new agent");
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

#[test]
fn orphan_session_marker_without_execution_lease_does_not_suppress_restore() {
    let config = AgentConfig {
        session_id: "agent-1".into(),
        provider: "codex".into(),
        resume_session: Some("resume-1".into()),
        ..Default::default()
    };
    let persisted_status = "Headless";
    let leases = Vec::new();

    assert_eq!(persisted_status, "Headless");
    assert!(!super::has_active_headless_execution_lease(
        &config,
        &leases,
        "2026-09-23T12:00:00Z"
    ));
}

#[test]
fn active_background_execution_lease_is_recognized_across_processes() {
    let config = AgentConfig {
        session_id: "agent-1".into(),
        provider: "codex".into(),
        resume_session: Some("resume-1".into()),
        ..Default::default()
    };
    let leases = vec![wardian_core::conversation_lease::ConversationLease {
        agent_id: "agent-1".into(),
        provider: "codex".into(),
        resume_session: "resume-1".into(),
        owner_kind: "automation_run".into(),
        owner_id: "run-1".into(),
        acquisition_id: "acquisition-1".into(),
        owner_node_id: Some("agent-1".into()),
        mode: "background_resume".into(),
        started_at: "2026-09-23T11:00:00Z".into(),
        heartbeat_at: "2026-09-23T11:59:00Z".into(),
        expires_at: "2026-09-23T12:20:00Z".into(),
    }];

    assert!(super::has_active_headless_execution_lease(
        &config,
        &leases,
        "2026-09-23T12:00:00Z"
    ));
}

#[test]
fn expired_background_lease_does_not_claim_headless_ownership() {
    let config = AgentConfig {
        session_id: "agent-1".into(),
        provider: "codex".into(),
        resume_session: Some("resume-1".into()),
        ..Default::default()
    };
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: "agent-1".into(),
        provider: "codex".into(),
        resume_session: "resume-1".into(),
        owner_kind: "automation_run".into(),
        owner_id: "run-1".into(),
        acquisition_id: "acquisition-1".into(),
        owner_node_id: Some("agent-1".into()),
        mode: "background_resume".into(),
        started_at: "2026-09-23T11:00:00Z".into(),
        heartbeat_at: "2026-09-23T11:30:00Z".into(),
        expires_at: "2026-09-23T11:59:00Z".into(),
    };
    assert!(!super::has_active_headless_execution_lease(
        &config,
        &[lease],
        "2026-09-23T12:00:00Z"
    ));
}
