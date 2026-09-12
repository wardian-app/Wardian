//! Cold initialization must remain observable and cancellable without a turn.
use super::*;

#[tokio::test]
async fn socket_wait_stays_pending_until_the_path_appears() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let checks = std::cell::Cell::new(0);
    let waiting = owner::wait_for_socket(
        &socket,
        tokio::time::Instant::now() + Duration::from_secs(60),
        || {
            checks.set(checks.get() + 1);
            Ok(())
        },
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
    tokio::time::timeout(Duration::from_secs(3), waiting)
        .await
        .unwrap()
        .unwrap();
    assert!(checks.get() >= 2);
    assert_eq!(std::fs::read(&socket).unwrap(), b"presence fixture");
}

#[tokio::test]
async fn socket_wait_checks_child_exit_before_accepting_a_present_path() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let alive = std::cell::Cell::new(true);
    let waiting = owner::wait_for_socket(
        &socket,
        tokio::time::Instant::now() + Duration::from_secs(60),
        || {
            if alive.get() {
                Ok(())
            } else {
                Err(CodexSharedError::unsupported("captured child exited"))
            }
        },
    );
    tokio::pin!(waiting);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    std::fs::write(&socket, b"presence fixture").unwrap();
    alive.set(false);
    let error = tokio::time::timeout(Duration::from_secs(3), waiting)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.message, "captured child exited");
    assert!(!error.provider_boundary_crossed);
    assert!(socket.exists());
}

#[tokio::test]
async fn socket_wait_rejects_a_missing_path_at_its_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let checks = std::cell::Cell::new(0);
    let error = tokio::time::timeout(
        Duration::from_secs(3),
        owner::wait_for_socket(&socket, tokio::time::Instant::now(), || {
            checks.set(checks.get() + 1);
            Ok(())
        }),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.message, "Codex local socket startup timed out");
    assert_eq!(checks.get(), 1);
    assert!(!error.provider_boundary_crossed);
    assert!(!socket.exists());
}

#[tokio::test]
async fn socket_wait_yields_to_caller_cancellation_without_advancing_startup() {
    use std::future::Future;
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("control.sock");
    let checks = std::cell::Cell::new(0);
    let advanced = std::cell::Cell::new(false);
    let (cancel, mut cancelled) = oneshot::channel::<()>();
    {
        let startup = async {
            owner::wait_for_socket(
                &socket,
                tokio::time::Instant::now() + Duration::from_secs(60),
                || {
                    checks.set(checks.get() + 1);
                    Ok(())
                },
            )
            .await?;
            advanced.set(true);
            Ok::<(), CodexSharedError>(())
        };
        tokio::pin!(startup);
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(startup.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        cancel.send(()).unwrap();
        // Match the owner's biased cancellation boundary. No helper-owned
        // process or task needs cleanup; owner process joining is tested elsewhere.
        tokio::select! {
            biased;
            result = &mut cancelled => result.unwrap(),
            result = &mut startup => panic!("socket wait advanced after cancellation: {result:?}"),
        }
    }
    assert_eq!(checks.get(), 1);
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
