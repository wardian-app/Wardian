use super::*;
use crate::control::test_support::TestWardianHome;
use crate::state::{ActiveAgent, AgentWatchState, AppState};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::Manager;
use wardian_core::models::AgentConfig;

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

fn test_agent(session_id: &str, provider: &str, runtime_generation: Option<u64>) -> ActiveAgent {
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

async fn seeded_state() -> (TestWardianHome, AppState) {
    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    {
        let mut agents = state.agents.lock().await;
        let codex = test_agent("codex-provisional", "codex", Some(7));
        codex
            .watch_state
            .lock()
            .expect("watch state lock")
            .set_codex_attachment_ready(false);
        agents.insert("codex-provisional".into(), codex);
        agents.insert(
            "claude-live".into(),
            test_agent("claude-live", "claude", None),
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
fn controllable_finalizer_promotes_only_the_exact_live_generation() {
    let ready = AtomicBool::new(false);
    let reader_alive = AtomicBool::new(true);
    let mut generation = Some(7);
    let mut status = "Booting".to_string();
    let mut calls = 0;
    complete_codex_attachment(
        &mut generation,
        7,
        &ready,
        &reader_alive,
        |_| {
            calls += 1;
            Ok(())
        },
        || status = "Idle".into(),
    )
    .expect("controllable finalizer should promote the live generation");
    assert_eq!(calls, 1);
    assert!(ready.load(Ordering::Acquire));
    assert_eq!(status, "Idle");

    ready.store(false, Ordering::Release);
    status = "Booting".into();
    let error = complete_codex_attachment(
        &mut generation,
        7,
        &ready,
        &reader_alive,
        |generation| {
            *generation = None;
            Ok(())
        },
        || status = "Idle".into(),
    )
    .expect_err("stale finalizer must not publish readiness");
    assert!(error.contains("stale after finalization"));
    assert!(!ready.load(Ordering::Acquire));
    assert_eq!(status, "Booting");

    generation = Some(7);
    reader_alive.store(true, Ordering::Release);
    let error = complete_codex_attachment(
        &mut generation,
        7,
        &ready,
        &reader_alive,
        |_| Ok(()),
        || {
            reader_alive.store(false, Ordering::Release);
            status = "Idle".into();
        },
    )
    .expect_err("reader EOF during readiness publication must roll back promotion");
    assert!(error.contains("readiness publication"));
    assert!(!ready.load(Ordering::Acquire));
    assert_eq!(status, "Idle");

    generation = Some(7);
    status = "Booting".into();
    reader_alive.store(true, Ordering::Release);
    let error = complete_codex_attachment(
        &mut generation,
        7,
        &ready,
        &reader_alive,
        |_| {
            reader_alive.store(false, Ordering::Release);
            Ok(())
        },
        || status = "Idle".into(),
    )
    .expect_err("reader EOF after the finalizer must roll back readiness");
    assert!(error.contains("reader exited"));
    assert!(!ready.load(Ordering::Acquire));
    assert_eq!(status, "Booting");

    generation = Some(7);
    reader_alive.store(true, Ordering::Release);
    let error = complete_codex_attachment(
        &mut generation,
        7,
        &ready,
        &reader_alive,
        |_| Err("provider finalizer failed".into()),
        || status = "Idle".into(),
    )
    .expect_err("finalizer failure must retain the provisional state");
    assert_eq!(error, "provider finalizer failed");
    assert!(!ready.load(Ordering::Acquire));
    assert_eq!(status, "Booting");
}

#[tokio::test]
async fn telemetry_cannot_publish_codex_ready_before_attachment() {
    let (_home, state) = seeded_state().await;
    let current_status = state
        .agents
        .lock()
        .await
        .get("codex-provisional")
        .expect("provisional Codex")
        .current_status
        .clone();
    let publication = crate::manager::publish_telemetry_status_observation(
        &state,
        &crate::manager::telemetry::TelemetryProviderStatus {
            session_id: "codex-provisional".into(),
            generation: 1,
            initial_status: "Booting".into(),
            initial_status_revision: state.status_revision("codex-provisional", &current_status),
            initial_status_intent_revision: state
                .status_intent_revision("codex-provisional", &current_status),
            status: "Idle".into(),
            transitions: vec![crate::manager::telemetry::TelemetryStatusTransition {
                previous_status: "Booting".into(),
                status: "Idle".into(),
                observed_at: "2026-09-27T12:00:00.000Z".into(),
            }],
            active_execution_conflict: false,
            current_status,
        },
    )
    .await;
    assert_eq!(
        publication.readiness,
        Some(wardian_core::control::ProviderInputReadiness::Unknown)
    );
    assert_eq!(publication.current_status.as_deref(), Some("Booting"));
}

#[tokio::test]
async fn status_contention_rechecks_codex_gate_without_dropping_other_providers() {
    let (home, state) = seeded_state().await;
    let app = tauri::test::mock_app();
    app.manage(state);
    let state = app.state::<AppState>();

    let (codex_status, claude_status) = {
        let agents = state.agents.lock().await;
        (
            agents
                .get("codex-provisional")
                .expect("provisional Codex")
                .current_status
                .clone(),
            agents
                .get("claude-live")
                .expect("live Claude")
                .current_status
                .clone(),
        )
    };

    let agents = state.agents.lock().await;
    let app_handle = app.handle().clone();
    assert_eq!(
        status_admission(&app_handle, "codex-provisional", &codex_status, "Idle"),
        CodexStatusAdmission::WaitForRoster
    );
    assert_eq!(
        status_admission(&app_handle, "claude-live", &claude_status, "Processing..."),
        CodexStatusAdmission::WaitForRoster
    );
    drop(agents);

    assert_eq!(
        status_admission(&app_handle, "codex-provisional", &codex_status, "Idle"),
        CodexStatusAdmission::Blocked
    );
    assert_eq!(
        status_admission(&app_handle, "claude-live", &claude_status, "Processing..."),
        CodexStatusAdmission::Allowed
    );
    drop(home);
}

#[tokio::test]
async fn blocked_codex_status_preserves_an_existing_status_revision() {
    let (home, state) = seeded_state().await;
    let app = tauri::test::mock_app();
    app.manage(state);
    let state = app.state::<AppState>();
    let current_status = state
        .agents
        .lock()
        .await
        .get("codex-provisional")
        .expect("provisional Codex")
        .current_status
        .clone();
    // Model a deferred status intent that has already reserved its revision.
    let pending_revision = {
        let _status = current_status.lock().unwrap();
        state.reserve_status_intent("codex-provisional", &current_status, "Processing...")
    };

    let admission =
        super::status_admission(app.handle(), "codex-provisional", &current_status, "Idle");
    assert_eq!(admission, super::CodexStatusAdmission::Blocked);
    assert_eq!(
        crate::manager::reserve_agent_status_intent(
            state.inner(),
            admission,
            "codex-provisional",
            &current_status,
            "Booting",
            "Idle",
        ),
        None,
        "blocked status must not reserve a newer intent"
    );

    assert_eq!(
        state.status_intent_revision("codex-provisional", &current_status),
        pending_revision
    );
    assert_eq!(*current_status.lock().unwrap(), "Booting");
    drop(home);
}

#[tokio::test]
async fn publishing_starting_does_not_supersede_a_newer_queued_transition() {
    let (home, state) = seeded_state().await;
    let app = tauri::test::mock_app();
    app.manage(state);
    let state = app.state::<AppState>();
    let current_status = state
        .agents
        .lock()
        .await
        .get("claude-live")
        .expect("live Claude")
        .current_status
        .clone();
    let app_handle = app.handle().clone();
    let agents = state.agents.lock().await;
    let admission =
        super::status_admission(&app_handle, "claude-live", &current_status, "Processing...");
    assert_eq!(admission, super::CodexStatusAdmission::WaitForRoster);
    let intent_revision = crate::manager::reserve_agent_status_intent(
        state.inner(),
        admission,
        "claude-live",
        &current_status,
        "Booting",
        "Processing...",
    )
    .expect("queued Processing intent");
    assert_eq!(
        crate::manager::commit_agent_status_publication(
            state.inner(),
            "claude-live",
            &current_status,
            "Booting",
        ),
        None,
        "publishing the older Starting value must preserve queued Processing"
    );
    assert_eq!(*current_status.lock().unwrap(), "Booting");
    drop(agents);
    let applied = super::apply_deferred_status_transition(
        state.inner(),
        "claude-live",
        &current_status,
        "Booting",
        intent_revision,
        "Processing...",
    )
    .await;
    assert_eq!(*current_status.lock().unwrap(), "Processing...");
    let (status_sequence, status_revision) =
        applied.expect("the newer queued transition should be applied");
    assert!(status_sequence > 0);
    assert_eq!(
        state.status_revision("claude-live", &current_status),
        status_revision
    );
    // MockRuntime covers the revision and transition decision. The production
    // Wry scheduler and persistence boundary is exercised by runtime tests.
    drop(home);
}

#[tokio::test]
async fn late_old_arc_intent_does_not_cancel_live_deferred_status() {
    let (home, state) = seeded_state().await;
    let app = tauri::test::mock_app();
    app.manage(state);
    let state = app.state::<AppState>();
    let (old_status, current_status) = {
        let mut agents = state.agents.lock().await;
        let old_status = agents
            .get("claude-live")
            .expect("original agent")
            .current_status
            .clone();
        let replacement = test_agent("claude-live", "claude", None);
        let current_status = replacement.current_status.clone();
        agents.insert("claude-live".into(), replacement);
        (old_status, current_status)
    };
    let app_handle = app.handle().clone();
    let agents = state.agents.lock().await;
    let live_admission =
        super::status_admission(&app_handle, "claude-live", &current_status, "Processing...");
    let live_intent_revision = crate::manager::reserve_agent_status_intent(
        state.inner(),
        live_admission,
        "claude-live",
        &current_status,
        "Booting",
        "Processing...",
    )
    .expect("live deferred status intent");
    let old_admission = super::status_admission(&app_handle, "claude-live", &old_status, "Idle");
    let old_intent_revision = crate::manager::reserve_agent_status_intent(
        state.inner(),
        old_admission,
        "claude-live",
        &old_status,
        "Booting",
        "Idle",
    )
    .expect("old Arc intent reservation");
    drop(agents);

    assert_eq!(
        state.status_intent_revision("claude-live", &current_status),
        live_intent_revision,
        "an old Arc intent must not replace the live Arc's revision"
    );
    assert_eq!(
        super::apply_deferred_status_transition(
            state.inner(),
            "claude-live",
            &old_status,
            "Booting",
            old_intent_revision,
            "Idle",
        )
        .await,
        None,
        "a deferred intent from a replaced Arc must be rejected"
    );
    let (status_sequence, status_revision) = super::apply_deferred_status_transition(
        state.inner(),
        "claude-live",
        &current_status,
        "Booting",
        live_intent_revision,
        "Processing...",
    )
    .await
    .expect("the accepted live status intent should apply");
    assert!(status_sequence > 0);
    assert_eq!(
        state.status_revision("claude-live", &current_status),
        status_revision
    );
    assert!(*old_status.lock().unwrap() == "Booting");
    assert!(state.status_revision("claude-live", &current_status) > old_intent_revision);
    drop(home);
}

#[tokio::test]
async fn provisional_status_arc_publishes_after_replacement_install() {
    let (home, state) = seeded_state().await;
    wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id: "claude-live",
        session_name: "claude-live",
        description: "",
        agent_class: "Coder",
        provider: "claude",
        workspace: None,
        project: None,
        is_off: false,
        created_at: None,
    })
    .expect("insert isolated persisted agent");
    let app = tauri::test::mock_app();
    app.manage(state);
    let state = app.state::<AppState>();
    let replacement = test_agent("claude-live", "claude", None);
    let current_status = replacement.current_status.clone();

    let admission = super::status_admission(
        app.handle(),
        "claude-live",
        &current_status,
        "Processing...",
    );
    let intent_revision = crate::manager::reserve_agent_status_intent(
        state.inner(),
        admission,
        "claude-live",
        &current_status,
        "Booting",
        "Processing...",
    )
    .expect("provisional status intent");
    let transition = crate::manager::apply_admitted_status_transition(
        state.inner(),
        "claude-live",
        &current_status,
        "Booting",
        intent_revision,
        "Processing...",
    )
    .expect("provisional status transition");
    assert_eq!(*current_status.lock().unwrap(), "Processing...");
    let provisional_revision = transition.1;
    assert!(provisional_revision > 0);

    state
        .agents
        .lock()
        .await
        .insert("claude-live".into(), replacement);
    let publication_revision = crate::manager::commit_agent_status_publication(
        state.inner(),
        "claude-live",
        &current_status,
        "Processing...",
    )
    .expect("the installed provisional status should publish");
    assert!(publication_revision > provisional_revision);
    // MockRuntime verifies the provisional Arc's publication decision. The
    // production Wry persistence boundary is covered by runtime publication tests.
    drop(home);
}
