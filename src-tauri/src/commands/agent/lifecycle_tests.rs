//! Retained lifecycle cleanup with fake children; no provider processes run.
use super::super::tests::make_test_agent;
use super::{stop_native_owner, PendingRuntime};
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
        std::env::set_var("WARDIAN_HOME", &home);
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
    }
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
        let pending = PendingRuntime::prepare(&config, &Arc::new(TerminalSessionBroker::default()))
            .unwrap()
            .attach(runtime);
        let mut installed = pending.installed();
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
