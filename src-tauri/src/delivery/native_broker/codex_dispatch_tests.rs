//! Process-free shared submission failure persistence regressions.
use super::*;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn shared_dispatch_closed_before_submit_persists_failure_without_replay() {
    assert_shared_submission_failure(false, false).await;
}

#[tokio::test]
async fn shared_dispatch_disconnect_before_ack_persists_uncertainty_without_replay() {
    assert_shared_submission_failure(true, false).await;
    // Even when the failure transition cannot be stored, the returned error
    // must retain the crossed boundary rather than permit fallback/replay.
    assert_shared_submission_failure(true, true).await;
}

async fn assert_shared_submission_failure(after_write: bool, fail_persistence: bool) {
    let _lock = crate::utils::wardian_test_env_lock_async().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.db");
        wardian_core::db::init_db_at_path(&db_path).unwrap();
        let broker = NativeDeliveryBroker::new();
        let record = broker
            .admit(NativeDeliveryAdmission {
                interaction_id: "shared-failure".into(),
                message_id: "native-message-id".into(),
                target_agent_id: "agent".into(),
                sender_agent_id: None,
                provider: "codex".into(),
                generation: 7,
                operation: NativeMessageOperation::StartTurn,
                caller_idempotency_key: None,
                parent_interaction_id: None,
                deadline_at: None,
                body: "literal ordinary input".into(),
            })
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let (server_error_tx, mut server_error_rx) =
            tokio::sync::mpsc::unbounded_channel::<String>();
        let server = tokio::spawn(async move {
            let result: Result<(), String> = async {
                let (stream, _) = listener
                    .accept()
                    .await
                    .map_err(|error| format!("initial server accept failed: {error}"))?;
                let mut socket = tokio_tungstenite::accept_async(stream)
                    .await
                    .map_err(|error| format!("WebSocket handshake failed: {error}"))?;
                if after_write {
                    let message = socket
                        .next()
                        .await
                        .ok_or_else(|| "client ended before turn/start request".to_string())?
                        .map_err(|error| format!("reading turn/start failed: {error}"))?;
                    let request: Value = serde_json::from_str(
                        message
                            .to_text()
                            .map_err(|error| format!("turn/start was not text: {error}"))?,
                    )
                    .map_err(|error| format!("turn/start JSON was invalid: {error}"))?;
                    if request["method"] != "turn/start" {
                        return Err(format!("unexpected method: {}", request["method"]));
                    }
                    let expected_params = json!({
                        "threadId":"owned",
                        "clientUserMessageId":"native-message-id",
                        "input":[{"type":"text","text":"literal ordinary input"}]
                    });
                    if request["params"] != expected_params {
                        return Err(format!(
                            "unexpected turn/start params: {}",
                            request["params"]
                        ));
                    }
                    if fail_persistence {
                        let connection = rusqlite::Connection::open(&db_path)
                            .map_err(|error| format!("SQLite trigger connection open failed: {error}"))?;
                        connection
                            .execute_batch(
                                "CREATE TRIGGER reject_failure_transition BEFORE INSERT ON native_deliveries
                                 WHEN NEW.phase = 'submitted_unconfirmed'
                                 BEGIN SELECT RAISE(FAIL, 'injected persistence failure'); END;",
                            )
                            .map_err(|error| {
                                format!("SQLite trigger installation failed: {error}")
                            })?;
                    }
                    socket
                        .close(None)
                        .await
                        .map_err(|error| format!("server close after turn/start failed: {error}"))?;
                } else {
                    // The client is deliberately closed before broker admission;
                    // no turn/start may precede the close frame.
                    match socket.next().await {
                        Some(Ok(Message::Close(_))) | None => {}
                        Some(Ok(message)) => {
                            return Err(format!(
                                "unexpected pre-submit message: {message:?}"
                            ));
                        }
                        Some(Err(error)) => {
                            return Err(format!("reading pre-submit close failed: {error}"));
                        }
                    }
                }
                match tokio::time::timeout(Duration::from_millis(50), listener.accept()).await {
                    Ok(Ok(_)) => Err("submission failure triggered a reconnect/replay".to_string()),
                    Ok(Err(error)) => Err(format!("reconnect probe failed: {error}")),
                    Err(_) => Ok(()),
                }
            }
            .await;
            if let Err(error) = &result {
                let _ = server_error_tx.send(error.clone());
            }
            result
        });
        let client = crate::delivery::codex_shared::CodexSharedClient::connect(
            "agent".into(),
            7,
            &endpoint,
            "test-owned-token",
        )
        .await
        .unwrap();
        client
            .bind(&json!({"thread":{
                "id":"owned","canAcceptDirectInput":true,"turns":[]
            }}))
            .unwrap();
        if !after_write {
            client.close().await;
        }
        let submission = broker.submit_shared_codex_input(&client, &record);
        tokio::pin!(submission);
        let failure = match tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                failure = &mut submission => {
                    failure.expect_err("shared submission unexpectedly succeeded")
                }
                server_error = server_error_rx.recv() => {
                    if let Some(error) = server_error {
                        panic!("shared submission fixture server failed: {error}");
                    }
                    submission
                        .await
                        .expect_err("shared submission unexpectedly succeeded")
                }
            }
        }).await {
            Ok(failure) => failure,
            Err(_) => {
                let server_state = if server.is_finished() {
                    match server.await {
                        Ok(Ok(())) => "server completed without reporting an error".to_string(),
                        Ok(Err(error)) => format!("server error: {error}"),
                        Err(error) => format!("server task join error: {error}"),
                    }
                } else {
                    "server task is still running".to_string()
                };
                panic!("shared submission fixture timed out after 5s; {server_state}");
            }
        };
        assert_eq!(failure.provider_boundary_crossed, after_write);
        let phase = if after_write {
            NativeDeliveryPhase::SubmittedUnconfirmed
        } else {
            NativeDeliveryPhase::FailedBeforeSubmit
        };
        let persisted = broker.get("shared-failure").unwrap();
        assert_eq!(persisted.envelope, record.envelope);
        assert!(persisted.provider_request_id.is_none());
        assert!(persisted.provider_turn_id.is_none());
        if fail_persistence {
            assert_eq!(failure.code, NativeDeliveryErrorCode::TransportUnavailable);
            assert!(failure.message.contains("injected persistence failure"));
            assert_eq!(persisted.phase, NativeDeliveryPhase::Dispatching);
        } else {
            assert_eq!(
                failure.code,
                if after_write {
                    NativeDeliveryErrorCode::SubmittedUnconfirmed
                } else {
                    NativeDeliveryErrorCode::CapabilityUnavailable
                }
            );
            assert!(!failure.message.is_empty());
            assert_eq!(persisted.phase, phase);
            assert_eq!(persisted.detail.as_deref(), Some(failure.message.as_str()));
        }
        // Inspect immediately, before owner cleanup or restart reconciliation.
        let evidence = broker.evidence("shared-failure", 100).unwrap();
        assert_eq!(evidence.len(), 3);
        let failed: Vec<_> = evidence
            .iter()
            .filter(|event| event.phase == phase)
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].source, NativeEvidenceSource::Caller);
        assert!(failed[0]
            .detail
            .as_deref()
            .is_some_and(|detail| !detail.is_empty()));
        assert!(failure
            .message
            .starts_with(failed[0].detail.as_deref().unwrap()));
        assert!(failed[0].provider_request_id.is_none());
        assert!(failed[0].provider_turn_id.is_none());
        assert!(!evidence.iter().any(|event| matches!(
            event.phase,
            NativeDeliveryPhase::ProviderAccepted | NativeDeliveryPhase::Completed
        )));
        client.close().await;
        match server.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("shared submission fixture server failed: {error}"),
            Err(error) => panic!("shared submission fixture server task failed: {error}"),
        }
    })
    .await
    .unwrap();
}
