//! Real loopback bridge failures with retained test child handles and real leases.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Default)]
struct ChildState {
    kills: AtomicUsize,
    exited: AtomicBool,
    kill_failed: AtomicBool,
    panic_once: AtomicBool,
}

#[derive(Debug)]
struct Child(Arc<ChildState>);

impl portable_pty::ChildKiller for Child {
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kills.fetch_add(1, Ordering::SeqCst);
        if self.0.kill_failed.load(Ordering::SeqCst) {
            Err(std::io::Error::other("owned test kill failure"))
        } else {
            Ok(())
        }
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(Self(self.0.clone()))
    }
}

impl portable_pty::Child for Child {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        if self.0.kills.load(Ordering::SeqCst) > 0 {
            assert!(
                !self.0.panic_once.swap(false, Ordering::SeqCst),
                "owned test wait panic"
            );
        }
        Ok(self
            .0
            .exited
            .load(Ordering::SeqCst)
            .then(|| portable_pty::ExitStatus::with_exit_code(0)))
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        panic!("startup cleanup must use bounded retained-handle observation")
    }

    fn process_id(&self) -> Option<u32> {
        None
    }

    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

async fn until(mut observed: impl FnMut() -> bool) {
    while !observed() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn fatal_pi_handshake_stops_only_its_child_and_keeps_lease_until_exit() {
    failure_case(Case::Normal).await;
}

#[tokio::test]
async fn cancelling_pi_startup_observer_does_not_cancel_owned_cleanup() {
    failure_case(Case::ObserverCancelled).await;
}

#[tokio::test]
async fn uncertain_pi_stop_keeps_exclusion_until_later_observed_exit() {
    failure_case(Case::KillFailed).await;
}

#[tokio::test]
async fn pi_cleanup_panic_retains_fence_and_can_observe_later_exit() {
    failure_case(Case::WaitPanic).await;
}

#[tokio::test]
async fn idle_pi_status_does_not_release_unauthenticated_startup() {
    failure_case(Case::EarlyIdle).await;
}

#[tokio::test]
async fn failed_pi_handshake_waits_for_registration_commit() {
    failure_case(Case::PendingRegistration).await;
}

#[tokio::test]
async fn pi_handshake_timeout_is_a_terminal_startup_failure() {
    failure_case(Case::HandshakeTimeout).await;
}

#[tokio::test]
async fn completed_pi_cancellation_is_observed_before_roster_publication() {
    failure_case(Case::Unpublished).await;
}

#[tokio::test]
async fn stale_pi_startup_failure_cannot_stop_or_publish_into_replacement() {
    failure_case(Case::Stale).await;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Normal,
    ObserverCancelled,
    KillFailed,
    WaitPanic,
    EarlyIdle,
    PendingRegistration,
    HandshakeTimeout,
    Unpublished,
    Stale,
}

async fn failure_case(case: Case) {
    let cancel_observer = case == Case::ObserverCancelled;
    let kill_failed = case == Case::KillFailed;
    let early_idle = case == Case::EarlyIdle;
    let _home = crate::control::test_support::TestWardianHome::new_async().await;
    let temp = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        provider: "pi".into(),
        resume_session: Some(uuid::Uuid::new_v4().to_string()),
        ..Default::default()
    };
    let lease = acquire_provider_spawn_lease(&config).expect("owned startup lease");
    let owner = lease.owner().clone();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let input = state
        .interactions
        .start_provider_input_generation(&config.session_id, ProviderInputReadiness::Booting, None)
        .await;
    let extension = temp.path().join("extension.mjs");
    std::fs::write(&extension, "// owned fixture\n").unwrap();
    let mut plan = state
        .native_delivery
        .prepare_pi_tui(
            crate::delivery::native_broker::NativeSessionSpec {
                target_agent_id: config.session_id.clone(),
                provider: "pi".into(),
                generation: input.generation,
                workspace: temp.path().to_path_buf(),
                config: config.clone(),
            },
            temp.path().join("session.jsonl"),
            extension,
        )
        .await
        .unwrap();
    plan.attached();
    let launch: serde_json::Value = serde_json::from_str(plan.config()).unwrap();
    let signals = Arc::new(ChildState::default());
    signals.kill_failed.store(kill_failed, Ordering::SeqCst);
    signals
        .panic_once
        .store(case == Case::WaitPanic, Ordering::SeqCst);
    let other = Arc::new(ChildState::default());
    let status = Arc::new(Mutex::new(
        if early_idle { "Idle" } else { "Starting" }.to_owned(),
    ));
    let mut agent = super::tests::agent_without_pty();
    agent.config = Arc::new(Mutex::new(config.clone()));
    agent.child_process = Some(Box::new(Child(signals.clone())));
    agent.runtime_generation = Some(7);
    agent.current_status = status.clone();
    let mut foreign = super::tests::agent_without_pty();
    foreign.child_process = Some(Box::new(Child(other.clone())));
    state
        .agents
        .lock()
        .await
        .insert(config.session_id.clone(), agent);
    state
        .agents
        .lock()
        .await
        .insert("other-agent".into(), foreign);

    let publication = Arc::new(RegistrationPublicationState::default());
    let cancelled = Arc::new(AtomicBool::new(false));
    let home = crate::utils::fs::get_wardian_home().unwrap();
    let displaced = if case == Case::Stale || case == Case::Unpublished {
        let old = state
            .agents
            .lock()
            .await
            .remove(&config.session_id)
            .unwrap();
        if case == Case::Unpublished {
            signals.exited.store(true, Ordering::SeqCst);
            let handle = crate::manager::codex_stop::prepare_pi_stop(&home, &config.session_id)
                .unwrap()
                .capture(old)
                .begin_stop();
            handle.wait().await.unwrap();
            cancelled.store(true, Ordering::SeqCst);
            None
        } else {
            let mut replacement = super::tests::agent_without_pty();
            replacement.config = Arc::new(Mutex::new(config.clone()));
            replacement.current_status = Arc::new(Mutex::new("Idle".into()));
            replacement.runtime_generation = Some(8);
            replacement.child_process = Some(Box::new(Child(other.clone())));
            state
                .agents
                .lock()
                .await
                .insert(config.session_id.clone(), replacement);
            Some(old)
        }
    } else {
        None
    };
    let watcher = release_provider_spawn_lease_after_readiness(
        lease,
        status.clone(),
        config.session_id.clone(),
        Arc::new(AtomicBool::new(false)),
        app.handle().clone(),
        7,
        SpawnPublicationGate {
            pi_bridge: Some(plan.owner()),
            unpublished_failure: Some(cancelled),
            registration: (case == Case::PendingRegistration).then(|| publication.clone()),
        },
    );
    if case == Case::Unpublished {
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .unwrap()
            .unwrap();
        assert!(!wardian_core::conversation_lease::load_leases_checked()
            .unwrap()
            .iter()
            .any(|entry| entry.owner() == owner));
        assert_eq!(signals.kills.load(Ordering::SeqCst), 0);
        other.exited.store(true, Ordering::SeqCst);
        return;
    }
    if early_idle {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(acquire_provider_spawn_lease(&config).is_err());
        assert_eq!(signals.kills.load(Ordering::SeqCst), 0);
    }
    let mut socket =
        tokio::net::TcpStream::connect(("127.0.0.1", launch["port"].as_u64().unwrap() as u16))
            .await
            .unwrap();
    // An actual invalid hello makes the production listener reject this startup.
    if case != Case::HandshakeTimeout {
        socket.write_all(&2_u32.to_be_bytes()).await.unwrap();
        socket.write_all(b"{}").await.unwrap();
    }
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(7), socket.read(&mut [0_u8; 1]))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    if case == Case::PendingRegistration {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(signals.kills.load(Ordering::SeqCst), 0);
        assert!(acquire_provider_spawn_lease(&config).is_err());
        publication.commit();
    }
    if let Some(displaced) = displaced {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(signals.kills.load(Ordering::SeqCst), 0);
        assert_eq!(other.kills.load(Ordering::SeqCst), 0);
        assert!(acquire_provider_spawn_lease(&config).is_err());
        // Owned rescue supplies exit proof without granting the stale monitor
        // any authority over the replacement runtime.
        let handle = crate::manager::codex_stop::prepare_pi_stop(&home, &config.session_id)
            .unwrap()
            .capture(displaced)
            .begin_stop();
        signals.exited.store(true, Ordering::SeqCst);
        handle.wait().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .unwrap()
            .unwrap();
        let agents = state.agents.lock().await;
        assert_eq!(
            *agents[&config.session_id].current_status.lock().unwrap(),
            "Idle"
        );
        assert_eq!(agents[&config.session_id].runtime_generation, Some(8));
        assert_eq!(other.kills.load(Ordering::SeqCst), 0);
        other.exited.store(true, Ordering::SeqCst);
        return;
    }
    let stopped = tokio::time::timeout(
        Duration::from_secs(2),
        until(|| signals.kills.load(Ordering::SeqCst) > 0),
    )
    .await;
    if stopped.is_err() {
        signals.exited.store(true, Ordering::SeqCst);
        watcher.abort();
        plan.owner().close();
        assert!(
            stopped.is_ok(),
            "fatal bridge failure left the owned Pi child Starting"
        );
    }
    assert_eq!(other.kills.load(Ordering::SeqCst), 0);
    if cancel_observer {
        watcher.abort();
    }
    if kill_failed || case == Case::WaitPanic {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            *state.agents.lock().await[&config.session_id]
                .current_status
                .lock()
                .unwrap(),
            "Action Needed"
        );
        let home = crate::utils::fs::get_wardian_home().unwrap();
        assert!(
            crate::manager::codex_stop::await_quiescent(&home, &config.session_id)
                .await
                .is_err()
        );
    }
    assert!(acquire_provider_spawn_lease(&config).is_err());
    // EOF/status alone is not proof that the retained child exited.
    *status.lock().unwrap() = "Off".into();
    assert!(acquire_provider_spawn_lease(&config).is_err());
    signals.exited.store(true, Ordering::SeqCst);
    if !cancel_observer {
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("startup cleanup completed")
            .unwrap();
    } else {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !wardian_core::conversation_lease::load_leases_checked()
                    .unwrap()
                    .iter()
                    .any(|entry| entry.owner() == owner)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("detached cleanup completed after observer cancellation");
    }
    assert!(!wardian_core::conversation_lease::load_leases_checked()
        .unwrap()
        .iter()
        .any(|entry| entry.owner() == owner));
    assert_eq!(
        *state.agents.lock().await[&config.session_id]
            .current_status
            .lock()
            .unwrap(),
        "Error"
    );
    assert_eq!(
        state.agents.lock().await[&config.session_id]
            .config
            .lock()
            .unwrap()
            .resume_session,
        config.resume_session
    );
    assert_eq!(other.kills.load(Ordering::SeqCst), 0);
    other.exited.store(true, Ordering::SeqCst);
    let replacement = acquire_provider_spawn_lease(&config).expect("ordinary recovery can acquire");
    drop(replacement);
}
