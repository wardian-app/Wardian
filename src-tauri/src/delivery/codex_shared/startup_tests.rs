//! Cold initialization must remain observable and cancellable without a turn.
use super::*;

#[tokio::test]
async fn socket_wait_stays_pending_until_the_path_appears() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let checks = std::cell::Cell::new(0);
    let waiting = owner::observe_socket_wait(
        &socket,
        tokio::time::Instant::now() + Duration::from_secs(60),
        std::future::pending(),
        || {
            checks.set(checks.get() + 1);
            owner::ChildLiveness::Alive
        },
        |_| false,
    );
    tokio::pin!(waiting);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert_eq!(checks.get(), 1);
    // A plain file deliberately exercises presence only, not socket validity,
    // proxy connection, initialize, or native TUI attachment.
    std::fs::write(&socket, b"presence fixture").unwrap();
    let diagnostic = tokio::time::timeout(Duration::from_secs(3), waiting)
        .await
        .unwrap();
    assert!(checks.get() >= 2);
    assert_eq!(diagnostic.outcome, owner::SocketWaitOutcome::Ready);
    assert_eq!(diagnostic.socket_presence, owner::SocketPresence::Present);
    assert_eq!(diagnostic.child_liveness, owner::ChildLiveness::Alive);
    let mut timings = owner::OwnerStartTimings::default();
    timings.record_socket_wait(diagnostic);
    let log_value = timings.socket_wait_diagnostic_value();
    assert_eq!(log_value["outcome"], "ready");
    assert_eq!(log_value["socket_presence"], "present");
    assert_eq!(log_value["child_liveness"], "alive");
    assert_eq!(std::fs::read(&socket).unwrap(), b"presence fixture");
}

#[tokio::test]
async fn socket_wait_checks_child_exit_before_accepting_a_present_path() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let alive = std::cell::Cell::new(true);
    let waiting = owner::observe_socket_wait(
        &socket,
        tokio::time::Instant::now() + Duration::from_secs(60),
        std::future::pending(),
        || {
            if alive.get() {
                owner::ChildLiveness::Alive
            } else {
                owner::ChildLiveness::Exited
            }
        },
        |_| false,
    );
    tokio::pin!(waiting);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    std::fs::write(&socket, b"presence fixture").unwrap();
    alive.set(false);
    let diagnostic = tokio::time::timeout(Duration::from_secs(3), waiting)
        .await
        .unwrap();
    assert_eq!(diagnostic.outcome, owner::SocketWaitOutcome::ChildExited);
    assert_eq!(diagnostic.socket_presence, owner::SocketPresence::Present);
    assert_eq!(diagnostic.child_liveness, owner::ChildLiveness::Exited);
    let mut timings = owner::OwnerStartTimings::default();
    timings.record_socket_wait(diagnostic);
    let log_value = timings.socket_wait_diagnostic_value();
    assert_eq!(log_value["outcome"], "child_exited");
    assert_eq!(log_value["socket_presence"], "present");
    assert_eq!(log_value["child_liveness"], "exited");
    let error = diagnostic.outcome.into_result().unwrap_err();
    assert_eq!(error.message, "captured Codex daemon is no longer alive");
    assert!(!error.provider_boundary_crossed);
    assert!(socket.exists());
}

#[tokio::test]
async fn socket_wait_timeout_records_elapsed_and_bounded_diagnostic() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("private-control.sock");
    let checks = std::cell::Cell::new(0);
    let diagnostic = owner::observe_socket_wait(
        &socket,
        tokio::time::Instant::now(),
        std::future::pending(),
        || {
            checks.set(checks.get() + 1);
            owner::ChildLiveness::Alive
        },
        |_| false,
    )
    .await;
    assert_eq!(diagnostic.outcome, owner::SocketWaitOutcome::TimedOut);
    assert_eq!(diagnostic.socket_presence, owner::SocketPresence::Missing);
    assert_eq!(diagnostic.child_liveness, owner::ChildLiveness::Alive);
    assert_eq!(checks.get(), 2);
    let mut timings = owner::OwnerStartTimings::default();
    timings.record_socket_wait(diagnostic);
    let log_value = timings.socket_wait_diagnostic_value();
    assert_eq!(log_value["outcome"], "timed_out");
    assert_eq!(log_value["socket_presence"], "missing");
    assert_eq!(log_value["child_liveness"], "alive");
    assert_eq!(
        log_value["elapsed_ms"].as_u64().unwrap() as u128,
        diagnostic.elapsed.as_millis()
    );
    assert_eq!(
        diagnostic.outcome.into_result().unwrap_err().message,
        "Codex local socket startup timed out"
    );
    assert!(!log_value.to_string().contains("private-control.sock"));
    assert!(!socket.exists());
}

#[tokio::test(start_paused = true)]
async fn observed_backfill_can_cross_the_ordinary_deadline_with_a_hard_limit() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let started = tokio::time::Instant::now();
    let deadline = started + STARTUP_TIMEOUT;
    let (_, completed) = tokio::join!(
        async {
            tokio::time::sleep_until(deadline + Duration::from_secs(10)).await;
            std::fs::write(&socket, b"presence fixture").unwrap();
        },
        owner::observe_socket_wait(
            &socket,
            deadline,
            std::future::pending(),
            || owner::ChildLiveness::Alive,
            |_| true,
        )
    );
    assert_eq!(completed.outcome, owner::SocketWaitOutcome::Ready);
    assert!(tokio::time::Instant::now() > deadline);

    std::fs::remove_file(&socket).unwrap();
    let cutoff = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    let expired = owner::observe_socket_wait(
        &socket,
        cutoff,
        std::future::pending(),
        || owner::ChildLiveness::Alive,
        |_| true,
    )
    .await;
    assert_eq!(expired.outcome, owner::SocketWaitOutcome::TimedOut);
    assert!(tokio::time::Instant::now() >= cutoff + Duration::from_secs(60));
}

#[tokio::test(start_paused = true)]
async fn loss_of_backfill_progress_ends_grace_before_the_hard_limit() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    let loss = deadline + Duration::from_secs(8);
    let expired = owner::observe_socket_wait(
        &socket,
        deadline,
        std::future::pending(),
        || owner::ChildLiveness::Alive,
        |now| now < loss,
    )
    .await;
    assert_eq!(expired.outcome, owner::SocketWaitOutcome::TimedOut);
    assert!(tokio::time::Instant::now() >= loss);
    assert!(tokio::time::Instant::now() < loss + Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn socket_first_seen_after_ordinary_deadline_requires_grace() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let waiting = owner::observe_socket_wait(
        &socket,
        tokio::time::Instant::now() + STARTUP_TIMEOUT,
        std::future::pending(),
        || owner::ChildLiveness::Alive,
        |_| false,
    );
    tokio::pin!(waiting);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    tokio::time::advance(STARTUP_TIMEOUT + Duration::from_secs(1)).await;
    std::fs::write(&socket, b"late presence fixture").unwrap();
    let observed = waiting.await;
    assert_eq!(observed.socket_presence, owner::SocketPresence::Present);
    assert_eq!(observed.outcome, owner::SocketWaitOutcome::TimedOut);
}

#[tokio::test(start_paused = true)]
async fn socket_present_after_hard_limit_is_rejected_even_if_wait_was_not_polled() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let waiting = owner::observe_socket_wait(
        &socket,
        tokio::time::Instant::now() + STARTUP_TIMEOUT,
        std::future::pending(),
        || owner::ChildLiveness::Alive,
        |_| true,
    );
    tokio::pin!(waiting);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    // Suspend the wait future across both deadlines before the path appears.
    tokio::time::advance(STARTUP_TIMEOUT + Duration::from_secs(61)).await;
    std::fs::write(&socket, b"late presence fixture").unwrap();
    let observed = waiting.await;
    assert_eq!(observed.socket_presence, owner::SocketPresence::Present);
    assert_eq!(observed.outcome, owner::SocketWaitOutcome::TimedOut);
}

#[tokio::test(start_paused = true)]
async fn cancellation_and_child_exit_preempt_socket_readiness_during_grace() {
    use std::cell::Cell;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    for (cancel_attempt, expected) in [
        (true, owner::SocketWaitOutcome::Cancelled),
        (false, owner::SocketWaitOutcome::ChildExited),
    ] {
        let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
        let child_alive = Cell::new(true);
        let (cancel, cancelled) = oneshot::channel::<()>();
        let mut cancel = Some(cancel);
        let (_, observed) = tokio::join!(
            async {
                tokio::time::sleep_until(deadline + Duration::from_secs(2)).await;
                std::fs::write(&socket, b"presence fixture").unwrap();
                if cancel_attempt {
                    cancel.take().unwrap().send(()).unwrap();
                } else {
                    child_alive.set(false);
                }
            },
            async {
                owner::observe_socket_wait(
                    &socket,
                    deadline,
                    async {
                        let _ = cancelled.await;
                    },
                    || {
                        if child_alive.get() {
                            owner::ChildLiveness::Alive
                        } else {
                            owner::ChildLiveness::Exited
                        }
                    },
                    |_| true,
                )
                .await
            }
        );
        assert_eq!(observed.outcome, expected);
        std::fs::remove_file(&socket).unwrap();
    }
}

#[tokio::test]
async fn socket_wait_yields_to_caller_cancellation_without_advancing_startup() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let checks = std::cell::Cell::new(0);
    let advanced = std::cell::Cell::new(false);
    let (cancel, cancelled) = oneshot::channel::<()>();
    {
        let startup = async {
            let diagnostic = owner::observe_socket_wait(
                &socket,
                tokio::time::Instant::now() + Duration::from_secs(60),
                async {
                    let _ = cancelled.await;
                },
                || {
                    checks.set(checks.get() + 1);
                    owner::ChildLiveness::Alive
                },
                |_| false,
            )
            .await;
            let mut timings = owner::OwnerStartTimings::default();
            timings.record_socket_wait(diagnostic);
            if diagnostic.outcome == owner::SocketWaitOutcome::Ready {
                advanced.set(true);
            }
            (diagnostic, timings.socket_wait_diagnostic_value())
        };
        tokio::pin!(startup);
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(startup.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        cancel.send(()).unwrap();
        let (diagnostic, log_value) = tokio::time::timeout(Duration::from_secs(3), startup)
            .await
            .unwrap();
        assert_eq!(diagnostic.outcome, owner::SocketWaitOutcome::Cancelled);
        assert_eq!(diagnostic.socket_presence, owner::SocketPresence::Missing);
        assert_eq!(diagnostic.child_liveness, owner::ChildLiveness::Alive);
        assert_eq!(log_value["outcome"], "cancelled");
        assert_eq!(log_value["socket_presence"], "missing");
        assert_eq!(log_value["child_liveness"], "alive");
    }
    assert_eq!(checks.get(), 2);
    assert!(!advanced.get());
    assert!(!socket.exists());
}

#[tokio::test]
async fn queued_initialize_waits_for_native_ack_without_reissuing_or_starting_work() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let home = tempfile::tempdir().unwrap();
        let expected = home.path().canonicalize().unwrap();
        let reported = expected.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let (accepted_tx, accepted_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let request: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "initialize");
            accepted_tx.send(()).unwrap();
            release_rx.await.unwrap();
            socket.send(Message::Text(json!({"id":request["id"],"result":{"userAgent":"wardian/0.154.0 (simulated server)","codexHome":reported}}).to_string().into())).await.unwrap();
            let notification: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(notification["method"], "initialized");
            assert!(socket.next().await.is_none_or(|value| matches!(value, Ok(Message::Close(_)))));
        });
        let client = CodexSharedClient::connect("agent".into(), 7, &endpoint, "owned-token").await.unwrap();
        let initialized = {
            let client = client.clone();
            tokio::spawn(async move { client.initialize_queued(&expected).await })
        };
        accepted_rx.await.unwrap();
        assert!(!initialized.is_finished());
        assert_eq!(client.observation.borrow().activity(), CodexTurnActivity::Pending);
        release_tx.send(()).unwrap();
        assert_eq!(initialized.await.unwrap().unwrap(), "0.154.0");
        assert!(client.observation.borrow().active_turn.is_none());
        client.close().await;
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn queued_initialize_fails_on_connection_exit_without_fabricating_readiness() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let request: Value =
                serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(request["method"], "initialize");
            socket.close(None).await.unwrap();
        });
        let client = CodexSharedClient::connect("agent".into(), 8, &endpoint, "owned-token")
            .await
            .unwrap();
        assert!(client.initialize_queued(home.path()).await.is_err());
        assert_eq!(
            client.observation.borrow().activity(),
            CodexTurnActivity::Closed
        );
        assert!(client.observation.borrow().provider_version.is_none());
        client.close().await;
        server.await.unwrap();
    })
    .await
    .unwrap();
}
