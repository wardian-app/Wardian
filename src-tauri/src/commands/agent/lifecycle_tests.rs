//! Retained lifecycle cleanup with fake children; no provider processes run.
use super::super::tests::make_test_agent;
use super::super::tests::use_isolated_resume_setting;
use super::super::{
    prepare_restored_config_for_spawn, prepare_resume_config, prepare_resume_config_for_runtime,
};
use wardian_core::models::provider::AgentProvider;
use wardian_core::models::{AgentConfig, AgentSessionPersistenceOverride};

use super::{stop_native_owner, stop_native_owner_with_before_capture, PendingRuntime};

#[test]
fn pending_pi_registration_and_spawn_do_not_publish_a_resume_session() {
    let mut config = wardian_core::models::AgentConfig {
        provider: "pi".into(),
        fresh_provider_session_id: Some("reserved".into()),
        is_off: true,
        ..Default::default()
    };
    let mut active = config.clone();
    super::sync_registered_provider_session(&mut config, &mut active, Some("reserved".into()));
    assert_eq!(config.resume_session, None);
    assert_eq!(active.pending_pi_session_id(), Some("reserved"));
    assert!(!super::promote_fresh_provider_session_fields(
        "pi",
        &mut active
    ));
    assert_eq!(active.resume_session, None);
    // Confirmed sessions retain their launch capture marker without becoming pending.
    active.resume_session = Some("reserved".into());
    assert!(super::promote_fresh_provider_session_fields(
        "pi",
        &mut active
    ));
    assert_eq!(
        active.fresh_provider_session_id.as_deref(),
        Some("reserved")
    );
    let value = serde_json::to_value(active).unwrap();
    assert!(value.get("fresh_provider_session_id").is_none());
}

#[test]
fn cleared_pi_identity_remains_pending_through_off_reload_until_history_confirmation() {
    let (_guard, _temp) = use_isolated_resume_setting();
    let mut config = AgentConfig {
        provider: "pi".into(),
        session_id: "wardian-agent".into(),
        ..Default::default()
    };
    super::super::prepare_clear_config(&mut config).unwrap();
    let reserved = config.fresh_provider_session_id.clone().unwrap();
    super::finalize_clear_provider_session_fields("pi", &mut config);
    assert_eq!(config.pending_pi_session_id(), Some(reserved.as_str()));
    config.is_off = true;
    let mut restored: AgentConfig =
        serde_json::from_value(serde_json::to_value(config).unwrap()).unwrap();
    prepare_resume_config(&mut restored).unwrap();
    let provider = crate::providers::pi::PiProvider::new();
    assert!(provider
        .get_spawn_args(&restored, true)
        .windows(2)
        .any(|pair| pair == ["--session-id", &reserved]));
    // The owned watcher is the only production transition which confirms this.
    restored.resume_session = Some(reserved.clone());
    let mut confirmed: AgentConfig =
        serde_json::from_value(serde_json::to_value(restored).unwrap()).unwrap();
    prepare_resume_config(&mut confirmed).unwrap();
    assert!(provider
        .get_spawn_args(&confirmed, true)
        .windows(2)
        .any(|pair| pair == ["--session", &reserved]));
    assert_eq!(confirmed.fresh_provider_session_id, None);
    std::env::remove_var("WARDIAN_HOME");
}

#[test]
fn clear_finalization_preserves_other_manual_provider_behavior() {
    for provider in ["claude", "gemini", "mock"] {
        let mut config = AgentConfig {
            provider: provider.into(),
            fresh_provider_session_id: Some("fresh".into()),
            ..Default::default()
        };
        super::finalize_clear_provider_session_fields(provider, &mut config);
        assert_eq!(config.resume_session.as_deref(), Some("fresh"));
        assert_eq!(config.fresh_provider_session_id, None);
    }
}

#[test]
fn pending_pi_wake_recovers_complete_history_written_before_watcher_publication() {
    let (_guard, _temp) = use_isolated_resume_setting();
    let config = AgentConfig {
        provider: "pi".into(),
        session_id: "agent-1".into(),
        is_off: true,
        fresh_provider_session_id: Some("reserved".into()),
        session_persistence: AgentSessionPersistenceOverride::Resume,
        ..Default::default()
    };
    let dir = crate::providers::pi::PiProvider::session_dir(&config.session_id).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("old-partial.jsonl"),
        "{\"type\":\"session\",\"id\":\"reserved\"}\n",
    )
    .unwrap();
    let complete = dir.join("complete.jsonl");
    std::fs::write(&complete, "{\"type\":\"session\",\"id\":\"reserved\"}\n{\"type\":\"message\",\"message\":{\"role\":\"assistant\"}}\n").unwrap();
    let mut saved: AgentConfig =
        serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
    prepare_resume_config(&mut saved).unwrap();
    assert_eq!(saved.resume_session.as_deref(), Some("reserved"));
    assert_eq!(saved.fresh_provider_session_id.as_deref(), Some("reserved"));
    let persisted = serde_json::to_value(&saved).unwrap();
    assert!(persisted.get("fresh_provider_session_id").is_none());
    assert_eq!(persisted["resume_session"], "reserved");
    // Repeated preparation is strict and retains the canonical SID, never a path.
    prepare_resume_config(&mut saved).unwrap();
    assert_eq!(saved.resume_session.as_deref(), Some("reserved"));
    std::fs::remove_file(complete).unwrap();
    assert_eq!(saved.pending_pi_session_id(), None);
    std::env::remove_var("WARDIAN_HOME");
}
use crate::delivery::{codex_shared::CodexSharedOwner, native_broker::NativeSessionSpec};
use crate::manager::codex_stop::{await_quiescent, retry_stop};
use crate::state::terminal_session::{
    TerminalBrokerError, TerminalRuntimeHandles, TerminalSessionBroker,
};
use crate::state::{ActiveAgent, AppState};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

#[derive(Debug, Default)]
struct ChildState {
    exited: AtomicBool,
    kills: AtomicUsize,
    drops: AtomicUsize,
}

#[derive(Debug, Clone)]
struct FakeChild(Arc<ChildState>);

impl portable_pty::ChildKiller for FakeChild {
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kills.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("retained fake kill failure"))
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

impl portable_pty::Child for FakeChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(self
            .0
            .exited
            .load(Ordering::SeqCst)
            .then(|| portable_pty::ExitStatus::with_exit_code(0)))
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        panic!("retained cleanup must poll without blocking wait")
    }
    fn process_id(&self) -> Option<u32> {
        None
    }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

impl Drop for FakeChild {
    fn drop(&mut self) {
        self.0.drops.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    workspace: PathBuf,
    old_home: Option<std::ffi::OsString>,
    old_codex_home: Option<std::ffi::OsString>,
    old_roots: Option<Vec<PathBuf>>,
    old_native: Option<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("legacy-home-needing-compaction".repeat(3));
        let workspace = temp.path().join("workspace");
        let native = temp.path().join("native");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&native).unwrap();
        let old_home = std::env::var_os("WARDIAN_HOME");
        let old_codex_home = std::env::var_os("CODEX_HOME");
        std::env::set_var("WARDIAN_HOME", &home);
        // The default source belongs to the private native-home fixture below.
        std::env::remove_var("CODEX_HOME");
        // If a regression bypasses the stop gate, the long home and empty root
        // list still reject startup before any provider can be launched.
        let old_roots =
            crate::utils::codex_home::TEST_ROOTS.with(|value| value.replace(Some(Vec::new())));
        let old_native = crate::utils::codex_messaging::TEST_NATIVE_HOME
            .with(|value| value.replace(Some(native)));
        Self {
            _temp: temp,
            home,
            workspace,
            old_home,
            old_codex_home,
            old_roots,
            old_native,
        }
    }

    fn runtime(&self, id: &str, child: &Arc<ChildState>) -> ActiveAgent {
        let mut agent = make_test_agent();
        {
            let mut config = agent.config.lock().unwrap();
            config.session_id = id.into();
            config.provider = "codex".into();
            config.agent_class.clear();
            config.folder = self.workspace.to_string_lossy().into_owned();
            config.is_off = true; // Status/absence of handles is not quiescence.
        }
        agent.runtime_generation = Some(7);
        agent.child_process = Some(Box::new(FakeChild(child.clone())));
        agent
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        crate::utils::codex_home::TEST_ROOTS.with(|value| value.replace(self.old_roots.take()));
        crate::utils::codex_messaging::TEST_NATIVE_HOME
            .with(|value| value.replace(self.old_native.take()));
        match self.old_home.take() {
            Some(home) => std::env::set_var("WARDIAN_HOME", home),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
        match self.old_codex_home.take() {
            Some(home) => std::env::set_var("CODEX_HOME", home),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }
}

#[tokio::test]
async fn codex_stop_preflight_error_does_not_install_hold_or_detach_runtime() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let fixture = Fixture::new();
    let id = "invalid/agent-id";
    let child = Arc::new(ChildState::default());
    let state = AppState::new();
    state
        .agents
        .lock()
        .await
        .insert(id.into(), fixture.runtime(id, &child));
    let callback_ran = AtomicBool::new(false);

    let error = stop_native_owner_with_before_capture(&state, id, false, |_| {
        callback_ran.store(true, Ordering::SeqCst);
        Ok(())
    })
    .await
    .unwrap_err();
    assert!(error.contains("one complete agent ID"), "{error}");
    assert!(!callback_ran.load(Ordering::SeqCst));
    let agents = state.agents.lock().await;
    assert!(agents[id].child_process.is_some());
    assert_eq!(agents[id].runtime_generation, Some(7));
    assert_eq!(child.kills.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_legacy_stop_blocks_normal_owner_preparation_until_explicit_retry() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let fixture = Fixture::new();
    let id = "11111111-1111-4111-8111-111111111219";
    let child = Arc::new(ChildState::default());
    let agent = fixture.runtime(id, &child);
    let config = agent.config.lock().unwrap().clone();
    let state = AppState::new();
    state.agents.lock().await.insert(id.into(), agent);
    let _lifecycle = state.lock_agent_lifecycle(id).await;
    let source = fixture.home.join("agents").join(id).join("habitat/.codex");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("history.jsonl"), "legacy history").unwrap();

    let error = stop_native_owner(&state, id, false).await.unwrap_err();
    assert!(error.contains("retained fake kill failure"), "{error}");
    let retained_config = {
        let agents = state.agents.lock().await;
        let agent = &agents[id];
        assert!(agent.child_process.is_none());
        assert!(agent.runtime_generation.is_none());
        agent.config.clone()
    };
    assert!(retained_config.lock().unwrap().is_off);
    assert_eq!(child.drops.load(Ordering::SeqCst), 0);
    let spec = NativeSessionSpec {
        target_agent_id: id.into(),
        provider: "codex".into(),
        generation: 8,
        workspace: fixture.workspace.clone(),
        config,
    };
    let error = CodexSharedOwner::start(&spec, std::future::pending())
        .await
        .unwrap_err();
    assert!(
        error.message.contains("retained fake kill failure"),
        "{error}"
    );
    assert!(!error.provider_boundary_crossed);
    assert_eq!(
        child.kills.load(Ordering::SeqCst),
        1,
        "startup must not retry a failed stop"
    );
    assert!(
        !source.parent().unwrap().join("AGENTS.md").exists(),
        "normal startup must stop before habitat preparation"
    );
    assert!(!fixture
        .home
        .join("agents")
        .join(id)
        .join(".wardian-codex-home.json")
        .exists());
    assert_eq!(
        std::fs::read_to_string(source.join("history.jsonl")).unwrap(),
        "legacy history"
    );

    child.exited.store(true, Ordering::SeqCst);
    assert!(
        await_quiescent(&fixture.home, id).await.is_err(),
        "failure remains fenced until explicit retry"
    );
    stop_native_owner(&state, id, false)
        .await
        .expect("explicit lifecycle retry joins the retained handle");
    assert_eq!(child.drops.load(Ordering::SeqCst), 1);
    await_quiescent(&fixture.home, id).await.unwrap();
    let error = CodexSharedOwner::start(&spec, std::future::pending())
        .await
        .unwrap_err();
    assert!(
        error.message.contains("No secure compact Codex home"),
        "{error}"
    );
    assert!(
        source.parent().unwrap().join("AGENTS.md").exists(),
        "joined startup now reaches preparation, then the process-free root guard"
    );
}

#[tokio::test]
async fn cancelled_uncommitted_runtime_retains_stop_and_allows_explicit_retry() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let fixture = Fixture::new();
    let id = "cancelled-uncommitted";
    let child = Arc::new(ChildState::default());
    let mut runtime = fixture.runtime(id, &child);
    let terminal = Arc::new(TerminalSessionBroker::default());
    let (generation, mut input) = attached_terminal(&terminal, id).await;
    runtime.runtime_generation = Some(generation);
    let config = runtime.config.lock().unwrap().clone();
    let pending = PendingRuntime::prepare(&config, &terminal).unwrap();
    await_quiescent(&fixture.home, id)
        .await
        .expect("preflight token must not block owner startup");
    let pending = pending.attach(runtime);
    let (ready, received) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        let _pending = pending;
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    received.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let error = await_quiescent(&fixture.home, id).await.unwrap_err();
    assert!(error.contains("retained fake kill failure"), "{error}");
    assert_eq!(child.drops.load(Ordering::SeqCst), 0);
    assert_terminal_closed(&terminal, id, &mut input).await;
    child.exited.store(true, Ordering::SeqCst);
    retry_stop(&fixture.home, id).unwrap();
    await_quiescent(&fixture.home, id).await.unwrap();
    assert_eq!(child.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_candidate_keeps_original_error_and_retained_cleanup_failure() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let fixture = Fixture::new();
    let id = "failed-candidate";
    let child = Arc::new(ChildState::default());
    let runtime = fixture.runtime(id, &child);
    let config = runtime.config.lock().unwrap().clone();
    let pending = PendingRuntime::prepare(&config, &Arc::new(TerminalSessionBroker::default()))
        .unwrap()
        .attach(runtime);
    let error = pending.stop_after_failure("commit failed".into()).await;
    assert!(error.starts_with("commit failed; Codex cleanup retained:"));
    assert_eq!(child.drops.load(Ordering::SeqCst), 0);
    child.exited.store(true, Ordering::SeqCst);
    retry_stop(&fixture.home, id).unwrap();
    await_quiescent(&fixture.home, id).await.unwrap();
}

#[tokio::test]
async fn installed_candidate_and_non_codex_runtime_do_not_create_stop_fences() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let fixture = Fixture::new();
    for provider in ["codex", "claude"] {
        let id = format!("installed-{provider}");
        let child = Arc::new(ChildState::default());
        let runtime = fixture.runtime(&id, &child);
        runtime.config.lock().unwrap().provider = provider.into();
        let config = runtime.config.lock().unwrap().clone();
        let mut pending =
            PendingRuntime::prepare(&config, &Arc::new(TerminalSessionBroker::default()))
                .unwrap()
                .attach(runtime);
        let mut installed = pending.take_runtime();
        await_quiescent(&fixture.home, &id).await.unwrap();
        assert_eq!(child.kills.load(Ordering::SeqCst), 0);
        child.exited.store(true, Ordering::SeqCst);
        // Avoid the normal safety-net kill in this synthetic successful install.
        drop(installed.child_process.take());
        installed.runtime_generation = None;
    }
}

async fn attached_terminal(
    broker: &TerminalSessionBroker,
    id: &str,
) -> (u64, tokio::sync::mpsc::Receiver<Vec<u8>>) {
    let (input, received) = tokio::sync::mpsc::channel(1);
    let generation = broker
        .start_or_replace_runtime(
            id,
            TerminalRuntimeHandles::new(input, |_| Ok(())),
            wardian_core::models::TerminalGeometry { rows: 24, cols: 80 },
        )
        .await
        .unwrap();
    broker
        .snapshot(id)
        .await
        .expect("attached terminal must be live");
    (generation, received)
}

async fn assert_terminal_closed(
    broker: &TerminalSessionBroker,
    id: &str,
    input: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    assert!(matches!(
        broker.snapshot(id).await,
        Err(TerminalBrokerError::RuntimeTerminated)
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), input.recv())
            .await
            .expect("terminal actor must release its input handle")
            .is_none()
    );
}

#[tokio::test]
async fn cancelled_attached_candidate_closes_terminal_before_releasing_stop_fence() {
    let _environment = crate::utils::wardian_test_env_lock_async().await;
    let fixture = Fixture::new();
    let id = "cancelled-after-attachment";
    let child = Arc::new(ChildState::default());
    // Process exit alone must not be treated as completed terminal cleanup.
    child.exited.store(true, Ordering::SeqCst);
    let mut runtime = fixture.runtime(id, &child);
    let terminal = Arc::new(TerminalSessionBroker::default());
    let (generation, mut input) = attached_terminal(&terminal, id).await;
    runtime.runtime_generation = Some(generation);
    let config = runtime.config.lock().unwrap().clone();
    let pending = PendingRuntime::prepare(&config, &terminal)
        .unwrap()
        .attach(runtime);
    let (ready, received) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        let _pending = pending;
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    received.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        await_quiescent(&fixture.home, id),
    )
    .await
    .expect("retained candidate cleanup must finish")
    .unwrap();
    assert_eq!(child.drops.load(Ordering::SeqCst), 1);
    assert_terminal_closed(&terminal, id, &mut input).await;

    // The old entry is gone. A later runtime is not a target of stale cleanup.
    let (replacement, mut replacement_input) = attached_terminal(&terminal, id).await;
    assert!(replacement > generation);
    retry_stop(&fixture.home, id).unwrap();
    await_quiescent(&fixture.home, id).await.unwrap();
    terminal
        .snapshot(id)
        .await
        .expect("replacement remains live");
    terminal.terminate_runtime(id, replacement).await.unwrap();
    assert_terminal_closed(&terminal, id, &mut replacement_input).await;
}

#[test]
fn pending_pi_off_reload_and_wake_preserve_reserved_identity_and_fresh_args() {
    let (_guard, _temp) = use_isolated_resume_setting();
    let original = AgentConfig {
        provider: "pi".into(),
        session_id: "wardian-agent".into(),
        fresh_provider_session_id: Some("reserved-pi-session".into()),
        is_off: true,
        ..Default::default()
    };
    for persistence in [
        AgentSessionPersistenceOverride::Resume,
        AgentSessionPersistenceOverride::Fresh,
    ] {
        let mut config: AgentConfig =
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
        config.session_persistence = persistence;
        prepare_restored_config_for_spawn(&mut config).unwrap();
        assert!(config.is_off);
        prepare_resume_config_for_runtime(&mut config, 0).unwrap();
        assert!(!config.is_off);
        assert_eq!(config.session_id, original.session_id);
        assert_eq!(config.pending_pi_session_id(), Some("reserved-pi-session"));
        let args = crate::providers::pi::PiProvider::new().get_spawn_args(&config, true);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session-id", "reserved-pi-session"]));
        assert!(!args.contains(&"--session".into()));
        // A failed start can be saved Off and retried with the same ID.
        config.is_off = true;
        let mut retry: AgentConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        prepare_resume_config(&mut retry).unwrap();
        assert_eq!(
            retry.pending_pi_session_id(),
            config.pending_pi_session_id()
        );
    }
    std::env::remove_var("WARDIAN_HOME");
}

#[test]
fn confirmed_and_legacy_pi_resume_remain_strict() {
    let (_guard, _temp) = use_isolated_resume_setting();
    let mut config = AgentConfig {
        provider: "pi".into(),
        session_id: "wardian-agent".into(),
        resume_session: Some("existing-pi-session".into()),
        fresh_provider_session_id: Some("existing-pi-session".into()),
        session_persistence: AgentSessionPersistenceOverride::Resume,
        ..Default::default()
    };
    prepare_resume_config(&mut config).unwrap();
    let args = crate::providers::pi::PiProvider::new().get_spawn_args(&config, true);
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--session", "existing-pi-session"]));
    assert!(!args.contains(&"--session-id".into()));
    config.resume_session = None;
    assert!(prepare_resume_config(&mut config).is_err());
    assert_eq!(config.fresh_provider_session_id, None);
    std::env::remove_var("WARDIAN_HOME");
}
