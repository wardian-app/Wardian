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
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            if after_write {
                let request: Value =
                    serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                assert_eq!(request["method"], "turn/start");
                assert_eq!(
                    request["params"],
                    json!({
                        "threadId":"owned",
                        "clientUserMessageId":"native-message-id",
                        "input":[{"type":"text","text":"literal ordinary input"}]
                    })
                );
                if fail_persistence {
                    rusqlite::Connection::open(&db_path).unwrap().execute_batch(
                        "CREATE TRIGGER reject_failure_transition BEFORE INSERT ON native_deliveries
                         WHEN NEW.phase = 'submitted_unconfirmed'
                         BEGIN SELECT RAISE(FAIL, 'injected persistence failure'); END;",
                    ).unwrap();
                }
                socket.close(None).await.unwrap();
            } else {
                // The client is deliberately closed before broker admission;
                // no turn/start may precede the close frame.
                assert!(matches!(
                    socket.next().await,
                    Some(Ok(Message::Close(_))) | None
                ));
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err(),
                "submission failure must not reconnect and replay"
            );
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
        let failure = broker
            .submit_shared_codex_input(&client, &record)
            .await
            .unwrap_err();
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
        server.await.unwrap();
    })
    .await
    .unwrap();
}
