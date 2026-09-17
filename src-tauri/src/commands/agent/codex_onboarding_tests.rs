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
use wardian_core::models::AgentConfig;

fn roster_without_session(configs: &[AgentConfig], session_id: &str) -> Vec<AgentConfig> {
    configs
        .iter()
        .filter(|config| config.session_id != session_id)
        .cloned()
        .collect()
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
    let state_for_update = state.clone();
    let mut same_session_update = tokio::spawn(async move {
        barrier_ready.send(()).expect("barrier owner signal");
        start_update_rx.await.expect("start same-session update");
        let result = super::super::update_agent_fields_in_state(
            &state_for_update,
            "codex-provisional",
            super::super::AgentUpdateFields {
                class: None,
                workspace: None,
                description: Some("same-session update while publishing"),
                model: None,
                reasoning_effort: None,
            },
            &[],
        )
        .await;
        drop(barrier);
        result.map(|outcome| outcome.config)
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
    other.config.lock().expect("other config lock").provider = "claude".into();
    state
        .agents
        .lock()
        .await
        .insert("claude-live".into(), other);
    state.agent_order.lock().await.push("claude-live".into());
    start_update.send(()).expect("start same-session update");
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
