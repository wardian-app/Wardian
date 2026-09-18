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
    let readiness = crate::manager::publish_telemetry_status_observation(
        &state,
        &crate::manager::telemetry::TelemetryProviderStatus {
            session_id: "codex-provisional".into(),
            generation: 1,
            status: "Idle".into(),
            current_status,
        },
    )
    .await;
    assert_eq!(
        readiness,
        wardian_core::control::ProviderInputReadiness::Unknown
    );
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
