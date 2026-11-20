//! Fake-child regressions and isolated Windows contained-tree acceptance.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use wardian_core::models::AgentConfig;

struct LeaseHome(Option<std::ffi::OsString>);

impl LeaseHome {
    fn set(path: &Path) -> Self {
        let previous = Self(std::env::var_os("WARDIAN_HOME"));
        std::env::set_var("WARDIAN_HOME", path);
        previous
    }
}

impl Drop for LeaseHome {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

fn prior_hold() -> wardian_core::conversation_lease::ConversationLease {
    wardian_core::conversation_lease::ConversationLease {
        agent_id: "held-agent".into(),
        provider: "codex".into(),
        resume_session: "old-session".into(),
        owner_kind: "prior_provider_hold".into(),
        owner_id: "prior-owner".into(),
        acquisition_id: "exact-acquisition".into(),
        owner_node_id: None,
        mode: "uncertain_previous_provider".into(),
        started_at: "2026-11-12T00:00:00Z".into(),
        heartbeat_at: "2026-11-12T00:00:00Z".into(),
        expires_at: "9999-12-31T23:59:59Z".into(),
    }
}

#[tokio::test]
async fn legacy_hold_without_runtime_evidence_is_never_released_online() {
    use wardian_core::conversation_lease::{load_leases_checked, save_leases};
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let home = tempfile::tempdir().unwrap();
    let _env = LeaseHome::set(home.path());
    let hold = prior_hold();
    save_leases(std::slice::from_ref(&hold)).unwrap();
    recover_prior_hold(home.path(), &hold.agent_id)
        .await
        .unwrap();
    assert_eq!(load_leases_checked().unwrap(), vec![hold]);
}

#[tokio::test]
async fn direct_child_exit_does_not_release_a_bound_prior_hold() {
    use wardian_core::conversation_lease::{load_leases_checked, save_leases};
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let home = tempfile::tempdir().unwrap();
    let _env = LeaseHome::set(home.path());
    let hold = prior_hold();
    save_leases(std::slice::from_ref(&hold)).unwrap();
    let signals = Arc::new(Signals::default());
    signals.exited.store(true, Ordering::SeqCst);
    signals.wait_failure.store(true, Ordering::SeqCst);
    let handle = prepare_stop(home.path(), &hold.agent_id)
        .unwrap()
        .with_prior_hold(Some(hold.owner()))
        .capture(agent(&hold.agent_id, &signals))
        .begin_stop();
    assert!(handle.wait().await.is_err());
    assert!(await_quiescent(home.path(), &hold.agent_id).await.is_err());
    assert_eq!(load_leases_checked().unwrap(), vec![hold.clone()]);
    assert_eq!(signals.drops.load(Ordering::SeqCst), 0);
    signals.wait_failure.store(false, Ordering::SeqCst);
    retry_stop(home.path(), &hold.agent_id).unwrap();
    handle.wait().await.unwrap();
    assert!(!handle.0.tree_exited.load(Ordering::Acquire));
    assert!(handle.observe_exit().unwrap());
    await_quiescent(home.path(), &hold.agent_id).await.unwrap();
    recover_prior_hold(home.path(), &hold.agent_id)
        .await
        .unwrap();
    assert_eq!(load_leases_checked().unwrap(), vec![hold]);
}

#[cfg(windows)]
#[tokio::test]
async fn terminal_job_exit_without_writer_evidence_retains_prior_hold() {
    use wardian_core::conversation_lease::{load_leases_checked, save_leases};
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let home = tempfile::tempdir().unwrap();
    let _env = LeaseHome::set(home.path());
    let hold = prior_hold();
    save_leases(std::slice::from_ref(&hold)).unwrap();
    let (job, child, _pty, _descendant) =
        crate::utils::process::test_contained_pty_tree(home.path()).await;
    let signals = Arc::new(Signals::default());
    let mut runtime = agent(&hold.agent_id, &signals);
    runtime.config.lock().unwrap().resume_session = Some(hold.resume_session.clone());
    runtime.process_id = child.process_id();
    runtime.child_process = Some(child);
    runtime.job_object = Some(job);
    let handle = prepare_stop(home.path(), &hold.agent_id)
        .unwrap()
        .with_prior_hold(Some(hold.owner()))
        .capture(runtime)
        .begin_stop();
    handle.wait().await.unwrap();
    assert!(handle.0.tree_exited.load(Ordering::Acquire));
    assert_eq!(load_leases_checked().unwrap(), vec![hold]);
}

#[cfg(windows)]
#[tokio::test]
async fn late_tui_exit_cannot_bypass_failed_native_owner_join_without_a_hold() {
    use crate::delivery::codex_shared::test_support::{stop_fixture, WriterCase};
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let home = tempfile::tempdir().unwrap();
    let _env = LeaseHome::set(home.path());
    let mut fixture = stop_fixture(
        home.path(),
        "late-owner",
        7,
        "thread",
        WriterCase::UnverifiedExit,
    )
    .await;
    let broker = Arc::new(crate::delivery::native_broker::NativeDeliveryBroker::new());
    broker
        .install_stop_test_owner("late-owner", 7, fixture.owner.clone())
        .await;
    let owner = broker
        .capture_codex_stop("late-owner", Some(7))
        .await
        .unwrap();
    let signals = Arc::new(Signals::default());
    signals.exited.store(true, Ordering::SeqCst);
    let handle = prepare_stop(home.path(), "late-owner")
        .unwrap()
        .with_codex_owner(owner)
        .unwrap()
        .capture(agent("late-owner", &signals))
        .begin_stop();
    assert!(handle.wait().await.is_err());
    assert!(!handle.observe_exit().unwrap());
    assert!(await_quiescent(home.path(), "late-owner").await.is_err());
    fixture.restore_containment().await;
    retry_stop(home.path(), "late-owner").unwrap();
    handle.wait().await.unwrap();
    fixture.cleanup().await;
}

#[cfg(windows)]
#[tokio::test]
async fn real_codex_tree_exit_after_cancellation_releases_only_bound_acquisition() {
    real_tree_release(
        false,
        crate::delivery::codex_shared::test_support::WriterCase::Owned,
    )
    .await;
}

#[cfg(windows)]
#[tokio::test]
async fn real_codex_tree_receipt_survives_lease_io_failure_for_online_retry() {
    real_tree_release(
        true,
        crate::delivery::codex_shared::test_support::WriterCase::Owned,
    )
    .await;
}

#[cfg(windows)]
#[tokio::test]
async fn daemon_hosted_thread_retains_hold_after_contained_tui_and_owner_exit() {
    real_tree_release(
        false,
        crate::delivery::codex_shared::test_support::WriterCase::DaemonHosted,
    )
    .await;
}

#[cfg(windows)]
#[tokio::test]
async fn missing_supported_unload_evidence_retains_hold_but_allows_fresh_rotation() {
    real_tree_release(
        false,
        crate::delivery::codex_shared::test_support::WriterCase::MissingUnload,
    )
    .await;
}

#[cfg(windows)]
#[tokio::test]
async fn unverified_owner_exit_retains_hold_and_exact_fence_until_retry() {
    real_tree_release(
        false,
        crate::delivery::codex_shared::test_support::WriterCase::UnverifiedExit,
    )
    .await;
}

#[cfg(windows)]
async fn real_tree_release(
    fail_release: bool,
    case: crate::delivery::codex_shared::test_support::WriterCase,
) {
    use crate::delivery::codex_shared::test_support::{stop_fixture, WriterCase};
    use std::os::windows::io::AsRawHandle;
    use wardian_core::conversation_lease::{load_leases_checked, save_leases};
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    let home = tempfile::tempdir().unwrap();
    let _env = LeaseHome::set(home.path());
    let hold = prior_hold();
    let mut later = hold.clone();
    later.acquisition_id = "later-acquisition".into();
    save_leases(&[hold.clone(), later.clone()]).unwrap();
    let history = home.path().join("history.jsonl");
    std::fs::write(&history, "retained history\n").unwrap();
    let mut fixture =
        stop_fixture(home.path(), &hold.agent_id, 7, &hold.resume_session, case).await;
    let broker = Arc::new(crate::delivery::native_broker::NativeDeliveryBroker::new());
    broker
        .install_stop_test_owner(&hold.agent_id, 7, fixture.owner.clone())
        .await;
    let owner = broker
        .capture_codex_stop(&hold.agent_id, Some(7))
        .await
        .unwrap();
    let (job, child, pty, descendant) =
        crate::utils::process::test_contained_pty_tree(home.path()).await;
    let signals = Arc::new(Signals::default());
    let mut runtime = agent(&hold.agent_id, &signals);
    runtime.config.lock().unwrap().resume_session = Some(hold.resume_session.clone());
    let config_before = runtime.config.lock().unwrap().clone();
    let config = runtime.config.clone();
    runtime.process_id = child.process_id();
    runtime.child_process = Some(child);
    runtime.job_object = Some(job);
    let guard = prepare_stop(home.path(), &hold.agent_id)
        .unwrap()
        .with_codex_owner(owner)
        .unwrap()
        .with_prior_hold(Some(hold.owner()))
        .capture(runtime);
    if fail_release {
        std::fs::write(
            wardian_core::conversation_lease::lease_path().unwrap(),
            "invalid-json",
        )
        .unwrap();
    }
    // Model cancellation during the broker await: Drop must finish the exact
    // retained stop and hold release without an observing request surviving.
    drop(guard);
    if fail_release || case == WriterCase::UnverifiedExit {
        assert!(await_quiescent(home.path(), &hold.agent_id).await.is_err());
        assert_eq!(entries(&key(home.path(), &hold.agent_id).unwrap()).len(), 1);
        assert_eq!(
            unsafe {
                winapi::um::synchapi::WaitForSingleObject(descendant.as_raw_handle() as _, 0)
            },
            winapi::um::winbase::WAIT_OBJECT_0
        );
        if fail_release {
            save_leases(&[hold.clone(), later.clone()]).unwrap();
        }
        fixture.restore_containment().await;
        recover_prior_hold(home.path(), &hold.agent_id)
            .await
            .unwrap();
    }
    await_quiescent(home.path(), &hold.agent_id).await.unwrap();
    recover_prior_hold(home.path(), &hold.agent_id)
        .await
        .unwrap();
    assert_eq!(
        load_leases_checked().unwrap(),
        if case == WriterCase::Owned {
            vec![later]
        } else {
            vec![hold, later]
        }
    );
    if let Some(writer) = fixture.external_writer.as_mut() {
        assert!(
            writer.try_wait().unwrap().is_none(),
            "an out-of-job writer survives both contained trees"
        );
    }
    assert_eq!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["method"] == "thread/unsubscribe")
            .count(),
        usize::from(case != WriterCase::DaemonHosted)
    );
    assert_eq!(
        unsafe { winapi::um::synchapi::WaitForSingleObject(descendant.as_raw_handle() as _, 0) },
        winapi::um::winbase::WAIT_OBJECT_0
    );
    let _listener = std::net::TcpListener::bind(pty.listening_addr).unwrap();
    assert_eq!(
        std::fs::read_to_string(history).unwrap(),
        "retained history\n"
    );
    assert_eq!(
        serde_json::to_value(&*config.lock().unwrap()).unwrap(),
        serde_json::to_value(config_before).unwrap()
    );
    fixture.cleanup().await;
}

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
