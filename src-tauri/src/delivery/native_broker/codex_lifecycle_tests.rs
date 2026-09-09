//! Process-free cancellation and prepared-owner admission regressions.
use super::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use tokio_tungstenite::tungstenite::Message;
use wardian_core::conversation_lease::{
    acquire_lease, release_lease_owner_persisted, ConversationLease, ConversationLeaseOwner,
};

fn spec(generation: u64) -> NativeSessionSpec {
    NativeSessionSpec {
        target_agent_id: "agent".into(),
        provider: "codex".into(),
        generation,
        workspace: PathBuf::new(),
        config: AgentConfig {
            session_id: "agent".into(),
            provider: "codex".into(),
            is_off: true,
            ..Default::default()
        },
    }
}

fn pause_creation(broker: &NativeDeliveryBroker, generation: u64) -> Arc<CodexCreationTestBarrier> {
    broker
        .codex_creation_test
        .reject_start
        .store(true, Ordering::Release);
    let barrier = Arc::new(CodexCreationTestBarrier::default());
    broker
        .codex_creation_test
        .before_gate
        .lock()
        .unwrap()
        .insert(generation, barrier.clone());
    barrier
}

#[tokio::test]
async fn agent_disposal_cancels_detached_creation_before_slot_registration() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let broker = Arc::new(NativeDeliveryBroker::new());
        let barrier = pause_creation(&broker, 7);
        let creating = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.create_shared_codex(spec(7)).await })
        };
        barrier.reached.notified().await;
        assert!(broker.sessions.lock().await.is_empty());
        // No externally held owner gate: disposal actually completes before the
        // detached creator is allowed to attempt registration.
        broker.dispose_agent("agent").await.unwrap();
        barrier.release.notify_one();
        let error = creating
            .await
            .unwrap()
            .expect_err("creation must be cancelled");
        assert!(error.message.contains("disposed") || error.message.contains("disposal"));
        assert!(!error.provider_boundary_crossed);
        assert_eq!(
            broker
                .codex_creation_test
                .start_attempts
                .load(Ordering::Acquire),
            0
        );
        assert!(broker.sessions.lock().await.is_empty());
    })
    .await
    .expect("detached creation cancellation must finish");
}

#[tokio::test]
async fn exact_generation_disposal_preserves_newer_detached_creation() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let broker = Arc::new(NativeDeliveryBroker::new());
        let old_barrier = pause_creation(&broker, 7);
        let new_barrier = pause_creation(&broker, 8);
        let old = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.create_shared_codex(spec(7)).await })
        };
        let newer = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.create_shared_codex(spec(8)).await })
        };
        old_barrier.reached.notified().await;
        new_barrier.reached.notified().await;
        broker.dispose_codex_generation("agent", 7).await.unwrap();
        old_barrier.release.notify_one();
        let error = old
            .await
            .unwrap()
            .expect_err("old generation must be cancelled");
        assert!(error.message.contains("disposed") || error.message.contains("disposal"));
        assert_eq!(
            broker
                .codex_creation_test
                .start_attempts
                .load(Ordering::Acquire),
            0
        );
        new_barrier.release.notify_one();
        let error = newer
            .await
            .unwrap()
            .expect_err("inert start sentinel must stop creation");
        assert_eq!(error.message, "test owner-start sentinel");
        assert!(!error.provider_boundary_crossed);
        assert_eq!(
            broker
                .codex_creation_test
                .start_attempts
                .load(Ordering::Acquire),
            1
        );
        assert!(broker.sessions.lock().await.is_empty());
    })
    .await
    .expect("exact-generation cancellation must finish");
}

struct LeaseFixture {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    _temp: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl LeaseFixture {
    async fn new() -> Self {
        let lock = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("WARDIAN_HOME");
        std::env::set_var("WARDIAN_HOME", temp.path());
        Self {
            _lock: lock,
            _temp: temp,
            previous,
        }
    }

    fn acquire(&self) -> ConversationLeaseOwner {
        let now = chrono::Utc::now();
        let lease = ConversationLease {
            agent_id: "agent".into(),
            provider: "codex".into(),
            resume_session: "owned".into(),
            owner_kind: "message_delivery".into(),
            owner_id: "test-owner".into(),
            acquisition_id: uuid::Uuid::new_v4().to_string(),
            owner_node_id: None,
            mode: "background_resume".into(),
            started_at: now.to_rfc3339(),
            heartbeat_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::minutes(20)).to_rfc3339(),
        };
        acquire_lease(lease.clone(), &now.to_rfc3339()).unwrap();
        lease.owner()
    }
}

impl Drop for LeaseFixture {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("WARDIAN_HOME", value),
            None => std::env::remove_var("WARDIAN_HOME"),
        }
    }
}

#[tokio::test]
async fn background_run_requires_existing_exact_owner_and_still_cleans_up() {
    let fixture = LeaseFixture::new().await;
    let lease = fixture.acquire();
    let broker = Arc::new(NativeDeliveryBroker::new());
    broker
        .codex_creation_test
        .reject_start
        .store(true, Ordering::Release);
    let pending = broker.codex_creations.register("agent", 7).unwrap();
    let error = broker
        .run_codex_background(
            spec(7),
            &lease,
            Vec::new(),
            "task",
            "context",
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("no shared Codex owner"));
    assert!(!error.provider_boundary_crossed);
    assert!(
        pending.is_cancelled(),
        "lookup failure must still reach exact disposal"
    );
    assert!(broker.sessions.lock().await.is_empty());

    // A newer registered slot is neither accepted nor removed by generation 7.
    let (tx, _rx) = mpsc::channel(1);
    broker.sessions.lock().await.insert(
        "agent".into(),
        NativeSessionHandle {
            generation: 8,
            provider: "codex".into(),
            capabilities: NativeTransportCapabilities::degraded("codex", "codex_app_server_ws"),
            tx,
            shared_codex: None,
            stopped: None,
        },
    );
    let error = broker
        .run_codex_background(
            spec(7),
            &lease,
            Vec::new(),
            "task",
            "context",
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("stale Codex owner generation"));
    assert!(!error.provider_boundary_crossed);
    assert_eq!(
        broker
            .sessions
            .lock()
            .await
            .get("agent")
            .unwrap()
            .generation,
        8
    );
    assert_eq!(
        broker
            .codex_creation_test
            .start_attempts
            .load(Ordering::Acquire),
        0
    );
    release_lease_owner_persisted(&lease).unwrap();
    broker.dispose_codex_generation("agent", 8).await.unwrap();
}

#[tokio::test]
async fn background_input_rechecks_lease_before_each_write_and_preserves_uncertainty() {
    let fixture = LeaseFixture::new().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        // Cover no first push, no first task, no second push, and no task after
        // an acknowledged push. Only the last two crossed a provider boundary.
        for (lost_before_input, contexts) in [(true, 0), (true, 1), (false, 2), (false, 1)] {
            let lease = fixture.acquire();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("ws://{}", listener.local_addr().unwrap());
            let server_lease = lease.clone();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                if !lost_before_input {
                    let request: Value = serde_json::from_str(
                        socket.next().await.unwrap().unwrap().to_text().unwrap(),
                    )
                    .unwrap();
                    assert_eq!(request["method"], "thread/inject_items");
                    // Remove exactly this acquisition before acknowledging the
                    // first append, deterministically fencing the next operation.
                    release_lease_owner_persisted(&server_lease).unwrap();
                    socket
                        .send(Message::Text(
                            json!({
                                "id":request["id"], "result":{}
                            })
                            .to_string()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                assert!(
                    matches!(socket.next().await, Some(Ok(Message::Close(_))) | None),
                    "lease loss must prevent any additional push or turn/start"
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
                    "id":"owned", "canAcceptDirectInput":true, "turns":[]
                }}))
                .unwrap();
            // Model the boundary after an owner/client has been obtained. The
            // production helper must revalidate instead of relying on lookup-time state.
            validate_background_lease(&spec(7), &lease).unwrap();
            if lost_before_input {
                release_lease_owner_persisted(&lease).unwrap();
            }
            let error = NativeDeliveryBroker::run_prepared_codex_background(
                &client,
                &spec(7),
                &lease,
                (0..contexts)
                    .map(|index| json!({"message":index}))
                    .collect(),
                "task",
                "context",
                Duration::from_secs(1),
            )
            .await
            .unwrap_err();
            assert!(error.message.contains("current explicit execution lease"));
            assert_eq!(error.provider_boundary_crossed, !lost_before_input);
            client.close().await;
            server.await.unwrap();
        }
    })
    .await
    .expect("lease-boundary tests must finish without a provider");
}
