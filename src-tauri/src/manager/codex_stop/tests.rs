//! Delayed/failing child handles only; these tests never start or stop a process.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use wardian_core::models::AgentConfig;

#[tokio::test]
async fn missing_pi_child_handle_cannot_become_late_exit_evidence() {
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let mut candidate = agent("missing-pi-child", &signals);
    candidate.config.lock().unwrap().provider = "pi".into();
    // The fixture retains the missing handle separately for owned teardown.
    // No real process or numeric PID exists in this invalid snapshot.
    let missing = candidate.child_process.take();
    let handle = prepare_pi_stop(home.path(), "missing-pi-child")
        .unwrap()
        .capture(candidate)
        .begin_stop();
    assert!(handle
        .wait()
        .await
        .unwrap_err()
        .contains("captured child handle is missing"));
    assert!(handle
        .observe_exit()
        .unwrap_err()
        .contains("captured child handle is missing"));
    assert_eq!(handle.provider_label(), "Pi");
    assert_eq!(signals.kills.load(Ordering::SeqCst), 0);
    assert_eq!(signals.drops.load(Ordering::SeqCst), 0);
    assert_eq!(
        entries(&key(home.path(), "missing-pi-child").unwrap()).len(),
        1
    );
    assert!(matches!(*handle.0.status.borrow(), Status::Failed(_)));

    // Restore only this no-process fixture's owned handle and prove its exit.
    handle
        .0
        .runtime
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .child_process = missing;
    signals.exited.store(true, Ordering::SeqCst);
    retry_stop(home.path(), "missing-pi-child").unwrap();
    handle.wait().await.unwrap();
    assert!(handle.observe_exit().unwrap());
    await_quiescent(home.path(), "missing-pi-child")
        .await
        .unwrap();
    assert_eq!(signals.drops.load(Ordering::SeqCst), 1);
}

#[derive(Debug, Default)]
struct Signals {
    exited: AtomicBool,
    kill_failure: AtomicBool,
    wait_failure: AtomicBool,
    panic_once: AtomicBool,
    kills: AtomicUsize,
    drops: AtomicUsize,
}

#[derive(Debug)]
struct FakeChild(Arc<Signals>);
#[derive(Debug)]
struct FakeKiller(Arc<Signals>);

impl portable_pty::ChildKiller for FakeKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kills.fetch_add(1, Ordering::SeqCst);
        if self.0.kill_failure.load(Ordering::SeqCst) {
            Err(std::io::Error::other("fake kill failure"))
        } else {
            Ok(())
        }
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(Self(self.0.clone()))
    }
}

impl portable_pty::ChildKiller for FakeChild {
    fn kill(&mut self) -> std::io::Result<()> {
        portable_pty::ChildKiller::kill(&mut FakeKiller(self.0.clone()))
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(FakeKiller(self.0.clone()))
    }
}

impl portable_pty::Child for FakeChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        assert!(
            !self.0.panic_once.swap(false, Ordering::SeqCst),
            "fake wait panic"
        );
        if self.0.wait_failure.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("fake wait failure"));
        }
        Ok(self
            .0
            .exited
            .load(Ordering::SeqCst)
            .then(|| portable_pty::ExitStatus::with_exit_code(0)))
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        panic!("stop worker must use bounded polling, not blocking child.wait")
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

fn agent(id: &str, signals: &Arc<Signals>) -> ActiveAgent {
    let config = AgentConfig {
        session_id: id.into(),
        provider: "codex".into(),
        ..AgentConfig::default()
    };
    ActiveAgent {
        config: Arc::new(Mutex::new(config)),
        child_process: Some(Box::new(FakeChild(signals.clone()))),
        background_processes: Vec::new(),
        memory_capability: None,
        runtime_generation: Some(7),
        process_id: None,
        query_count: Arc::new(Mutex::new(0)),
        init_timestamp: Arc::new(Mutex::new(None)),
        last_query_timestamp: Arc::new(Mutex::new(None)),
        current_status: Arc::new(Mutex::new("Idle".into())),
        last_status_at: Arc::new(Mutex::new(None)),
        watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            id.into(),
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

async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("fake child transition");
}

async fn fenced(home: &Path, id: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(20), await_quiescent(home, id))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn validation_only_registration_drop_leaves_no_fence() {
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let registration = prepare_stop(home.path(), "agent").unwrap();
    let independent = prepare_stop(other.path(), "agent").unwrap();
    await_quiescent(&home.path().join("."), "agent")
        .await
        .unwrap();
    drop(registration);
    await_quiescent(home.path(), "agent").await.unwrap();
    drop(independent);
    await_quiescent(other.path(), "agent").await.unwrap();
    assert!(prepare_stop(home.path(), "../bad").is_err());
}

#[tokio::test]
async fn capture_fences_before_stop_and_successful_kill_is_not_observed_exit() {
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let registration = prepare_stop(home.path(), "agent").unwrap();
    let guard = registration.capture(agent("agent", &signals));
    assert_eq!(signals.kills.load(Ordering::SeqCst), 0);
    fenced(home.path(), "agent").await;
    let handle = guard.begin_stop();
    until(|| signals.kills.load(Ordering::SeqCst) == 1).await;
    fenced(home.path(), "agent").await;
    assert_eq!(signals.drops.load(Ordering::SeqCst), 0);
    signals.exited.store(true, Ordering::SeqCst);
    handle.wait().await.unwrap();
    await_quiescent(home.path(), "agent").await.unwrap();
    assert_eq!(signals.drops.load(Ordering::SeqCst), 1);
    let next = Arc::new(Signals::default());
    let replacement = prepare_stop(home.path(), "agent")
        .unwrap()
        .capture(agent("agent", &next));
    drop(handle); // An old observer cannot remove the replacement's fence.
    fenced(home.path(), "agent").await;
    next.exited.store(true, Ordering::SeqCst);
    replacement.begin_stop().wait().await.unwrap();
}

#[tokio::test]
async fn cancellation_during_broker_await_drops_guard_but_retains_runtime_until_exit() {
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let registration = prepare_stop(home.path(), "agent").unwrap();
    let runtime = agent("agent", &signals);
    let (captured, received) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        let guard = registration.capture(runtime); // First operation after ownership transfer.
        captured.send(()).unwrap();
        std::future::pending::<()>().await; // Simulated broker shutdown await.
        drop(guard);
    });
    received.await.unwrap();
    assert_eq!(signals.kills.load(Ordering::SeqCst), 0);
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    until(|| signals.kills.load(Ordering::SeqCst) == 1).await;
    fenced(home.path(), "agent").await;
    assert_eq!(signals.drops.load(Ordering::SeqCst), 0);
    signals.exited.store(true, Ordering::SeqCst);
    await_quiescent(home.path(), "agent").await.unwrap();
    assert_eq!(signals.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelling_the_last_stop_waiter_does_not_cancel_cleanup() {
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let handle = prepare_stop(home.path(), "agent")
        .unwrap()
        .capture(agent("agent", &signals))
        .begin_stop();
    let waiter = tokio::spawn(async move { handle.wait().await });
    until(|| signals.kills.load(Ordering::SeqCst) == 1).await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    fenced(home.path(), "agent").await;
    assert_eq!(signals.drops.load(Ordering::SeqCst), 0);
    signals.exited.store(true, Ordering::SeqCst);
    await_quiescent(home.path(), "agent").await.unwrap();
    assert_eq!(signals.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn kill_wait_and_panic_failures_retain_same_handle_for_explicit_retry() {
    for failure in ["kill", "wait", "panic"] {
        let home = tempfile::tempdir().unwrap();
        let signals = Arc::new(Signals::default());
        match failure {
            "kill" => signals.kill_failure.store(true, Ordering::SeqCst),
            "wait" => signals.wait_failure.store(true, Ordering::SeqCst),
            _ => signals.panic_once.store(true, Ordering::SeqCst),
        }
        let handle = prepare_stop(home.path(), "agent")
            .unwrap()
            .capture(agent("agent", &signals))
            .begin_stop();
        assert!(handle.wait().await.is_err());
        assert!(await_quiescent(home.path(), "agent").await.is_err());
        assert_eq!(signals.drops.load(Ordering::SeqCst), 0);
        let previous_kills = signals.kills.load(Ordering::SeqCst);
        signals.kill_failure.store(false, Ordering::SeqCst);
        signals.wait_failure.store(false, Ordering::SeqCst);
        retry_stop(home.path(), "agent").unwrap();
        until(|| signals.kills.load(Ordering::SeqCst) > previous_kills).await;
        signals.exited.store(true, Ordering::SeqCst);
        handle.wait().await.unwrap();
        await_quiescent(home.path(), "agent").await.unwrap();
        assert_eq!(signals.drops.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn timeout_retains_fence_even_if_child_later_exits_until_explicit_retry() {
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let handle = prepare_with_timeout(home.path(), "agent", Duration::ZERO)
        .unwrap()
        .capture(agent("agent", &signals))
        .begin_stop();
    assert!(handle.wait().await.unwrap_err().contains("Timed out"));
    signals.exited.store(true, Ordering::SeqCst);
    assert!(await_quiescent(home.path(), "agent").await.is_err());
    assert_eq!(signals.drops.load(Ordering::SeqCst), 0);
    retry_stop(home.path(), "agent").unwrap();
    handle.wait().await.unwrap();
    assert_eq!(signals.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn every_captured_runtime_for_same_key_must_exit_and_other_homes_are_independent() {
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let first = Arc::new(Signals::default());
    let second = Arc::new(Signals::default());
    let one = prepare_stop(home.path(), "agent")
        .unwrap()
        .capture(agent("agent", &first))
        .begin_stop();
    let two = prepare_stop(&home.path().join("."), "agent")
        .unwrap()
        .capture(agent("agent", &second))
        .begin_stop();
    await_quiescent(other.path(), "agent").await.unwrap();
    first.exited.store(true, Ordering::SeqCst);
    one.wait().await.unwrap();
    fenced(home.path(), "agent").await;
    assert_eq!(second.drops.load(Ordering::SeqCst), 0);
    second.exited.store(true, Ordering::SeqCst);
    two.wait().await.unwrap();
    await_quiescent(home.path(), "agent").await.unwrap();
    assert_eq!(first.drops.load(Ordering::SeqCst), 1);
    assert_eq!(second.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn opencode_join_failure_keeps_exact_runtime_and_blocks_replacement() {
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let candidate = agent("opencode-failed-join", &signals);
    candidate.config.lock().unwrap().provider = "opencode".into();
    let handle = prepare_runtime_stop(home.path(), "opencode-failed-join", "opencode")
        .unwrap()
        .capture(candidate)
        .begin_stop();
    assert!(handle.wait().await.is_err());
    assert_eq!(handle.provider_label(), "OpenCode");
    assert!(await_quiescent(home.path(), "opencode-failed-join")
        .await
        .is_err());
    assert!(handle.0.runtime.lock().unwrap().is_some());
    assert_eq!(signals.kills.load(Ordering::SeqCst), 0);
    assert_eq!(signals.drops.load(Ordering::SeqCst), 0);
    // This invalid no-process fixture has no Windows job to recover. Teardown
    // releases only its own fake handle and registry entry.
    signals.exited.store(true, Ordering::SeqCst);
    drop(handle.0.runtime.lock().unwrap().take());
    release(&handle.0);
}

#[cfg(windows)]
#[tokio::test]
async fn opencode_cancelled_pause_retains_and_joins_exact_job_before_replacement() {
    use std::os::windows::io::AsRawHandle;
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let (job, child, _pty, descendant) =
        crate::utils::process::test_contained_pty_tree(home.path()).await;
    let mut candidate = agent("opencode-cancelled-pause", &signals);
    candidate.config.lock().unwrap().provider = "opencode".into();
    candidate.child_process = Some(child);
    candidate.job_object = Some(job);
    let registration =
        prepare_runtime_stop(home.path(), "opencode-cancelled-pause", "opencode").unwrap();
    let (captured, ready) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        let guard = registration.capture(candidate);
        captured.send(()).unwrap();
        std::future::pending::<()>().await;
        drop(guard);
    });
    ready.await.unwrap();
    fenced(home.path(), "opencode-cancelled-pause").await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(
        Duration::from_secs(10),
        await_quiescent(home.path(), "opencode-cancelled-pause"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        unsafe { winapi::um::synchapi::WaitForSingleObject(descendant.as_raw_handle() as _, 0) },
        winapi::um::winbase::WAIT_OBJECT_0
    );
    assert!(entries(&key(home.path(), "opencode-cancelled-pause").unwrap()).is_empty());
}

#[cfg(windows)]
#[derive(Debug)]
struct UncertainOpenCodeWait {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    fail: Arc<AtomicBool>,
}

#[cfg(windows)]
impl portable_pty::ChildKiller for UncertainOpenCodeWait {
    fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        self.child.clone_killer()
    }
}

#[cfg(windows)]
impl portable_pty::Child for UncertainOpenCodeWait {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("injected uncertain OpenCode wait"));
        }
        self.child.try_wait()
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        self.child.wait()
    }
    fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        self.child.as_raw_handle()
    }
}

#[cfg(windows)]
#[tokio::test]
async fn opencode_uncertain_job_join_retains_fence_until_explicit_same_handle_retry() {
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let (job, child, _pty, _descendant) =
        crate::utils::process::test_contained_pty_tree(home.path()).await;
    let fail = Arc::new(AtomicBool::new(true));
    let mut candidate = agent("opencode-uncertain-join", &signals);
    candidate.config.lock().unwrap().provider = "opencode".into();
    candidate.child_process = Some(Box::new(UncertainOpenCodeWait {
        child,
        fail: fail.clone(),
    }));
    candidate.job_object = Some(job);
    let handle = prepare_runtime_stop(home.path(), "opencode-uncertain-join", "opencode")
        .unwrap()
        .capture(candidate)
        .begin_stop();
    assert!(handle
        .wait()
        .await
        .unwrap_err()
        .contains("injected uncertain OpenCode wait"));
    assert!(await_quiescent(home.path(), "opencode-uncertain-join")
        .await
        .is_err());
    {
        let runtime = handle.0.runtime.lock().unwrap();
        let retained = runtime.as_ref().unwrap();
        assert!(retained.child_process.is_some());
        assert!(retained.job_object.is_some());
    }
    fail.store(false, Ordering::SeqCst);
    assert!(handle
        .observe_exit()
        .unwrap_err()
        .contains("root-only exit"));
    {
        let runtime = handle.0.runtime.lock().unwrap();
        let retained = runtime.as_ref().unwrap();
        assert!(retained.child_process.is_some());
        assert!(retained.job_object.is_some());
        // A later roster provider edit must not rewrite the captured policy or
        // identity, nor allow a different provider to bypass this session fence.
        retained.config.lock().unwrap().provider = "claude".into();
    }
    assert!(await_quiescent(home.path(), "opencode-uncertain-join")
        .await
        .is_err());
    retry_stop(home.path(), "opencode-uncertain-join").unwrap();
    handle.wait().await.unwrap();
    await_quiescent(home.path(), "opencode-uncertain-join")
        .await
        .unwrap();
    assert!(entries(&key(home.path(), "opencode-uncertain-join").unwrap()).is_empty());
}

#[cfg(windows)]
#[tokio::test]
async fn contained_capture_missing_child_retains_policy_until_same_handle_retry() {
    use std::os::windows::io::AsRawHandle;
    let home = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let (job, child, _pty, descendant) =
        crate::utils::process::test_contained_pty_tree(home.path()).await;
    let id = "contained-missing-child";
    let mut candidate = agent(id, &signals);
    candidate.child_process = Some(child);
    candidate.job_object = Some(job);
    // The ordinary Codex registration must capture the same contained policy.
    let guard = prepare_stop(home.path(), id).unwrap().capture(candidate);
    let entry = entries(&key(home.path(), id).unwrap()).pop().unwrap();
    let retained_child = entry
        .runtime
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .child_process
        .take();
    let handle = guard.begin_stop();
    assert!(handle.wait().await.unwrap_err().contains("retained child"));
    assert!(handle
        .observe_exit()
        .unwrap_err()
        .contains("root-only exit"));
    assert!(await_quiescent(home.path(), id).await.is_err());
    assert_eq!(
        unsafe { winapi::um::synchapi::WaitForSingleObject(descendant.as_raw_handle() as _, 0) },
        winapi::shared::winerror::WAIT_TIMEOUT
    );
    entry
        .runtime
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .child_process = retained_child;
    retry_stop(home.path(), id).unwrap();
    handle.wait().await.unwrap();
    await_quiescent(home.path(), id).await.unwrap();
    assert_eq!(
        unsafe { winapi::um::synchapi::WaitForSingleObject(descendant.as_raw_handle() as _, 0) },
        winapi::um::winbase::WAIT_OBJECT_0
    );
}

#[cfg(windows)]
#[tokio::test]
async fn cancellation_before_terminal_setup_captures_exact_starting_job() {
    use std::os::windows::io::AsRawHandle;
    for provider in ["opencode", "codex"] {
        let home = tempfile::tempdir().unwrap();
        let config = AgentConfig {
            session_id: "early-startup-cancel".into(),
            provider: provider.into(),
            ..Default::default()
        };
        let starting = StartingRuntime::prepare(
            home.path(),
            &config,
            Arc::new(TerminalSessionBroker::default()),
        )
        .unwrap();
        // This fixture has no terminal generation or published ActiveAgent.
        let (job, child, pty, descendant) =
            crate::utils::process::test_contained_pty_tree(home.path()).await;
        let starting = starting.attach(child, Some(job));
        let (ready, received) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            let _starting = starting;
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        received.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        tokio::time::timeout(
            Duration::from_secs(15),
            await_quiescent(home.path(), &config.session_id),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            unsafe {
                winapi::um::synchapi::WaitForSingleObject(descendant.as_raw_handle() as _, 0)
            },
            winapi::um::winbase::WAIT_OBJECT_0,
            "{provider}"
        );
        let _rebound = std::net::TcpListener::bind(pty.listening_addr).unwrap();
    }
}

#[tokio::test]
async fn never_launched_starting_guard_leaves_no_runtime_fence() {
    let home = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        session_id: "never-launched".into(),
        provider: "codex".into(),
        ..Default::default()
    };
    let starting = StartingRuntime::prepare(
        home.path(),
        &config,
        Arc::new(TerminalSessionBroker::default()),
    )
    .unwrap();
    drop(starting);
    await_quiescent(home.path(), &config.session_id)
        .await
        .unwrap();
    assert!(entries(&key(home.path(), &config.session_id).unwrap()).is_empty());
}
