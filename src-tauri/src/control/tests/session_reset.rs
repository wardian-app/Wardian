// Exercise the clear/worktree delivery boundary with owned mock Codex transports.
use super::*;
use crate::delivery::codex_shared::test_support::{stop_fixture, WriterCase};
use crate::delivery::codex_shared::CodexSharedClient;
use crate::delivery::native_broker::NativeSessionSpec;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use wardian_core::control::ReplyStatus;
use wardian_core::db::agent_messaging as store;

const RECIPIENT: &str = "reset-recipient";
const SENDER: &str = "reset-sender";
const OLD_THREAD: &str = "reset-old-thread";
const NEW_THREAD: &str = "reset-new-thread";
const OLD_PENDING: &str = "old pending conversation context";
const OLD_DELIVERED: &str = "old delivered conversation context";
const OLD_SETTLED: &str = "old settled task context";
const NEW_TASK: &str = "only the new task λ\r\n";
const DEADLINE: Duration = Duration::from_secs(5);

const EXCLUDED_THREAD: &str = "2d1f06c5-b097-422f-a2b7-06ccdb362e40";
const EARLIER_THREAD: &str = "7126d806-e526-440d-8dd0-19d9e4b86242";
const FRESH_THREAD: &str = "8d73bba5-8bc4-44e3-aa8d-3e7ca1de10c7";

fn cleared_codex_original() -> AgentConfig {
    let mut config = AgentConfig {
        session_id: RECIPIENT.into(),
        provider: "codex".into(),
        resume_session: Some(EXCLUDED_THREAD.into()),
        fresh_provider_session_id: Some(EXCLUDED_THREAD.into()),
        ..Default::default()
    };
    config
        .codex_config_mut_preserve_encoding()
        .cleared_provider_sessions = vec![EARLIER_THREAD.into()];
    config.codex_cleared_provider_sessions = vec![EARLIER_THREAD.into()];
    config
}

#[test]
fn clear_codex_replacement_preserves_prior_identity_exclusions() {
    for nested in [false, true] {
        let mut original = cleared_codex_original();
        if nested {
            original.mark_provider_config_nested_for_save();
        }
        let fresh = crate::commands::agent::prepare_clear_replacement_config(&original).unwrap();
        assert!(!fresh.is_off);
        assert!(fresh.resume_session.is_none());
        assert!(fresh.fresh_provider_session_id.is_none());
        let expected = vec![EARLIER_THREAD.to_owned(), EXCLUDED_THREAD.to_owned()];
        assert_eq!(fresh.codex_config().cleared_provider_sessions, expected);
        assert_eq!(fresh.codex_cleared_provider_sessions, expected);
        assert_eq!(original.resume_session.as_deref(), Some(EXCLUDED_THREAD));
    }
}

#[test]
fn clear_codex_replacement_rejects_excluded_identity_publication() {
    let original = cleared_codex_original();
    let mut replacement =
        crate::commands::agent::prepare_clear_replacement_config(&original).unwrap();
    let result =
        crate::manager::apply_provider_identity("codex", &mut replacement, EXCLUDED_THREAD);
    assert!(
        result.is_err(),
        "production clear's identity publisher accepted an excluded thread: {result:?}"
    );
    assert!(replacement.resume_session.is_none());
    assert!(replacement.fresh_provider_session_id.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn clear_codex_replacement_rejects_excluded_loaded_thread() {
    assert_clear_codex_attachment(EXCLUDED_THREAD, false).await;
}

#[test]
fn clear_codex_replacement_accepts_fresh_identity_publication() {
    let original = cleared_codex_original();
    let mut replacement =
        crate::commands::agent::prepare_clear_replacement_config(&original).unwrap();
    assert_eq!(
        crate::manager::apply_provider_identity("codex", &mut replacement, FRESH_THREAD).unwrap(),
        crate::manager::session_identity::ProviderIdentityOutcome::Captured,
    );
    assert_eq!(replacement.resume_session.as_deref(), Some(FRESH_THREAD));
    assert_eq!(
        replacement.fresh_provider_session_id.as_deref(),
        Some(FRESH_THREAD)
    );
    assert_eq!(
        crate::manager::apply_provider_identity("codex", &mut replacement, FRESH_THREAD).unwrap(),
        crate::manager::session_identity::ProviderIdentityOutcome::Confirmed,
    );
    assert_eq!(
        replacement.codex_config().cleared_provider_sessions,
        vec![EARLIER_THREAD.to_owned(), EXCLUDED_THREAD.to_owned()],
    );
}

#[tokio::test(flavor = "current_thread")]
async fn clear_codex_replacement_accepts_fresh_loaded_thread() {
    assert_clear_codex_attachment(FRESH_THREAD, true).await;
}

async fn assert_clear_codex_attachment(loaded_thread: &str, allowed: bool) {
    use crate::delivery::codex_shared::CodexSharedError;

    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    let mut original = cleared_codex_original();
    original.folder = home
        .path()
        .join("new-worktree")
        .to_string_lossy()
        .into_owned();
    let mut replacement =
        crate::commands::agent::prepare_clear_replacement_config(&original).unwrap();
    assert!(replacement.resume_session.is_none());
    assert!(replacement
        .codex_config()
        .cleared_provider_sessions
        .iter()
        .any(|id| id == EXCLUDED_THREAD));
    if allowed {
        // Normal clear captures its fresh bootstrap ID before starting the TUI.
        crate::manager::apply_provider_identity("codex", &mut replacement, loaded_thread).unwrap();
    }
    let mut agent = test_agent(RECIPIENT, RECIPIENT, "Test");
    agent.process_id = None;
    *agent.config.lock().unwrap() = replacement.clone();
    let config = agent.config.clone();
    state.agents.lock().await.insert(RECIPIENT.into(), agent);
    let generation = state
        .interactions
        .start_provider_input_generation(RECIPIENT, ProviderInputReadiness::Booting, None)
        .await
        .generation;
    let owner_directory = home.path().join("replacement-owner");
    std::fs::create_dir(&owner_directory).unwrap();
    let mut owner = stop_fixture(
        &owner_directory,
        RECIPIENT,
        generation,
        loaded_thread,
        WriterCase::Owned,
    )
    .await;
    // A fresh private owner is unbound. Exercise the actual attachment selector
    // using the mock TUI's supported loaded-list response, with no thread/start.
    let (client, peer) = reset_peer_with_binding(generation, loaded_thread, false).await;
    owner.owner.client.close().await;
    let unpublished_owner = Arc::get_mut(&mut owner.owner).unwrap();
    unpublished_owner.client = client;
    // Owner::start captures both fields from this same replacement config.
    unpublished_owner.attachment.expected_resume_id = replacement.resume_session.clone();
    unpublished_owner.attachment.cleared_provider_sessions =
        replacement.codex_config().cleared_provider_sessions;
    state
        .native_delivery
        .install_stop_test_owner(RECIPIENT, generation, None, owner.owner.clone())
        .await;

    let result = tokio::time::timeout(
        DEADLINE,
        state.native_delivery.finalize_codex_tui(
            RECIPIENT,
            generation,
            || async { Ok(()) },
            |id| {
                let mut captured = config.lock().unwrap();
                // This is the same identity publisher used by synchronous and
                // deferred production startup, including its existing validation.
                crate::manager::apply_provider_identity("codex", &mut captured, id)
                    .map(|_| ())
                    .map_err(CodexSharedError::unsupported)
            },
        ),
    )
    .await
    .expect("bounded production native attachment");
    let published = config.lock().unwrap().resume_session.clone();
    let requests = peer.requests.lock().unwrap().clone();
    state
        .native_delivery
        .dispose_agent(RECIPIENT)
        .await
        .unwrap();
    owner.cleanup().await;
    peer.finish().await;

    assert!(
        requests
            .iter()
            .any(|request| request["method"] == "thread/loaded/list"),
        "the actual attachment selector must inspect the mock TUI's loaded thread"
    );
    if allowed {
        let receipt = result.expect("non-excluded fresh bootstrap identity must attach");
        assert_eq!(receipt.delivery_state, "owner_ready");
        assert_eq!(receipt.provider_session_id, loaded_thread);
        assert_eq!(receipt.generation, generation);
        assert_eq!(published.as_deref(), Some(loaded_thread));
        for method in ["thread/inject_items", "thread/resume"] {
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| request["method"] == method)
                    .count(),
                1,
                "fresh attachment must complete normal context and metadata preparation",
            );
        }
        return;
    }
    assert!(
        result.is_err(),
        "fresh clear/worktree startup accepted an explicitly excluded retained thread: {result:?}"
    );
    assert!(
        published.is_none(),
        "excluded thread was published as the fresh replacement identity"
    );
    assert!(
        !requests.iter().any(|request| matches!(
            request["method"].as_str(),
            Some("thread/inject_items" | "thread/resume")
        )),
        "excluded thread must be rejected before startup context or metadata resume"
    );
}

struct ResetPeer {
    requests: Arc<Mutex<Vec<Value>>>,
    changed: Arc<Notify>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl ResetPeer {
    async fn wait_for(&self, method: &str) {
        tokio::time::timeout(DEADLINE, async {
            loop {
                if self
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|v| v["method"] == method)
                {
                    return;
                }
                self.changed.notified().await;
            }
        })
        .await
        .expect("the old owner's RPC must actually be pending before reset");
    }

    async fn finish(mut self) {
        let mut server = self.server.take().unwrap();
        let result = tokio::time::timeout(DEADLINE, &mut server).await;
        if result.is_err() {
            server.abort();
        }
        result
            .expect("owned peer must exit after disposal")
            .expect("mock peer protocol");
    }
}

impl Drop for ResetPeer {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

async fn reset_peer(generation: u64, thread: &str) -> (Arc<CodexSharedClient>, ResetPeer) {
    reset_peer_with_binding(generation, thread, true).await
}

async fn reset_peer_with_binding(
    generation: u64,
    thread: &str,
    bind_started_thread: bool,
) -> (Arc<CodexSharedClient>, ResetPeer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let (client, socket) = tokio::join!(
        CodexSharedClient::connect(RECIPIENT.into(), generation, &endpoint, "reset-test-token"),
        async {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream).await.unwrap()
        }
    );
    let client = client.expect("connect generation-bound mock owner");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let changed = Arc::new(Notify::new());
    let observed = changed.clone();
    let thread = thread.to_owned();
    let server = tokio::spawn(async move {
        let mut socket = socket;
        let mut unloaded = false;
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let request: Value = serde_json::from_str(&text).unwrap();
            recorded.lock().unwrap().push(request.clone());
            observed.notify_one();
            let result = match request["method"].as_str().expect("host RPC") {
                "thread/start" => json!({"thread":{
                    "id":thread,"canAcceptDirectInput":true,"status":{"type":"idle"},"turns":[]
                }}),
                // Hold a real outbound request across disposal. This is an RPC
                // shutdown check, not a substitute for a native receive/wait call_id.
                "thread/read" => continue,
                "thread/loaded/list" => {
                    json!({"data":if unloaded {vec![]} else {vec![thread.clone()]},"nextCursor":null})
                }
                "thread/unsubscribe" => {
                    unloaded = true;
                    json!({"status":"unsubscribed"})
                }
                "thread/inject_items" => json!({}),
                "thread/resume" => json!({
                    "thread":{"id":thread,"canAcceptDirectInput":true,
                        "status":{"type":"idle"},"turns":[]},
                    "approvalPolicy":"on-request","sandbox":{"type":"workspaceWrite"},
                    "approvalsReviewer":"user"
                }),
                "turn/start" => json!({"turn":{"id":"reset-new-turn"}}),
                other => panic!("unexpected reset-fixture RPC: {other}"),
            };
            if socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    if bind_started_thread {
        // This mode supplies separate transports for the context-isolation test.
        // Fresh startup policy is exercised by the unbound attachment test below.
        let started = client.request("thread/start", json!({})).await.unwrap();
        client
            .bind(&started)
            .expect("bind newly started mock thread");
    }
    (
        client,
        ResetPeer {
            requests,
            changed,
            server: Some(server),
        },
    )
}

async fn admit_reset_message(
    state: &AppState,
    generation: u64,
    body: &str,
    task: bool,
) -> store::Admitted {
    state
        .interactions
        .admit_agent_message(store::Admission {
            sender: SENDER,
            recipient: RECIPIENT,
            message: body,
            idempotency_key: None,
            task,
            generation,
        })
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn mock_session_reset_fences_context_without_advancing_generation() {
    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    for id in [SENDER, RECIPIENT] {
        let mut agent = test_agent(id, id, "Test");
        agent.process_id = None;
        agent.config.lock().unwrap().folder = home.path().to_string_lossy().into_owned();
        state.agents.lock().await.insert(id.into(), agent);
    }
    let generation = state
        .interactions
        .start_provider_input_generation(RECIPIENT, ProviderInputReadiness::Ready, None)
        .await
        .generation;
    let old = admit_reset_message(&state, generation, OLD_PENDING, false).await;
    let lifecycle = state.lock_agent_lifecycle(RECIPIENT).await;
    let (codex_stopped, original) =
        crate::commands::agent::reset_agent_session_delivery(&state, RECIPIENT, |_| Ok(None))
            .await
            .unwrap();
    drop(lifecycle);
    assert!(!codex_stopped);
    assert_eq!(original.provider, "mock");
    assert_eq!(
        state
            .interactions
            .current_provider_input_generation(RECIPIENT)
            .await,
        Some(generation)
    );
    assert!(store::with_db(|conn| {
        let highwater = store::information_highwater(conn, RECIPIENT)?;
        store::pending_information(conn, RECIPIENT, highwater)
    })
    .unwrap()
    .is_empty());
    let preserved = store::with_db(|conn| store::load(conn, &old.record.id)).unwrap();
    assert_eq!(
        serde_json::to_value(preserved).unwrap(),
        serde_json::to_value(old.record).unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_session_reset_first_task_excludes_old_context_and_closes_pending_rpc() {
    let home = TestWardianHome::new_async().await;
    let state = AppState::new();
    for id in [SENDER, RECIPIENT] {
        let mut agent = test_agent(id, id, "Test");
        agent.process_id = None;
        {
            let mut config = agent.config.lock().unwrap();
            config.provider = "codex".into();
            config.folder = home.path().to_string_lossy().into_owned();
            config.resume_session = Some(OLD_THREAD.into());
            config.is_off = false;
        }
        *agent.current_status.lock().unwrap() = "Idle".into();
        state.agents.lock().await.insert(id.into(), agent);
    }
    let old_generation = state
        .interactions
        .start_provider_input_generation(RECIPIENT, ProviderInputReadiness::Ready, None)
        .await
        .generation;
    let old_directory = home.path().join("old-owner");
    std::fs::create_dir(&old_directory).unwrap();
    let mut old_owner = stop_fixture(
        &old_directory,
        RECIPIENT,
        old_generation,
        OLD_THREAD,
        WriterCase::Owned,
    )
    .await;
    let (old_client, old_peer) = reset_peer(old_generation, OLD_THREAD).await;
    old_owner.owner.client.close().await;
    Arc::get_mut(&mut old_owner.owner)
        .expect("fixture owns unpublished owner")
        .client = old_client.clone();
    state
        .native_delivery
        .install_stop_test_owner(RECIPIENT, old_generation, None, old_owner.owner.clone())
        .await;

    let settled = admit_reset_message(&state, old_generation, OLD_SETTLED, true).await;
    let settled_claim = state
        .interactions
        .claim_agent_task(RECIPIENT, old_generation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled_claim.record.id, settled.record.id);
    state
        .interactions
        .reply_agent_message(
            RECIPIENT,
            &settled.record.id,
            ReplyStatus::Done,
            "explicit old result",
        )
        .await
        .unwrap();
    state
        .interactions
        .finish_agent_task(&settled_claim, "provider_completed")
        .await
        .unwrap();

    let delivered = admit_reset_message(&state, old_generation, OLD_DELIVERED, false).await;
    let delivered_claim = state
        .interactions
        .claim_agent_information(RECIPIENT, &delivered.record.id, old_generation)
        .await
        .unwrap()
        .unwrap();
    let old_context =
        store::with_db(|conn| store::message_context(conn, &delivered.record)).unwrap();
    let old_context = serde_json::to_string(&old_context).unwrap();
    let receipt = state
        .native_delivery
        .codex_push(
            RECIPIENT,
            old_generation,
            &delivered.record.id,
            &old_context,
        )
        .await
        .unwrap();
    state
        .interactions
        .finish_agent_task(&delivered_claim, &receipt.delivery_state)
        .await
        .unwrap();
    admit_reset_message(&state, old_generation, OLD_PENDING, false).await;

    let pending_client = old_client.clone();
    let pending_rpc = tokio::spawn(async move {
        pending_client
            .request("thread/read", json!({"threadId":OLD_THREAD}))
            .await
    });
    old_peer.wait_for("thread/read").await;
    let lifecycle = state.lock_agent_lifecycle(RECIPIENT).await;
    let (codex_stopped, mut replacement) =
        crate::commands::agent::reset_agent_session_delivery(&state, RECIPIENT, |_| Ok(None))
            .await
            .expect("production clear/worktree delivery reset");
    assert!(codex_stopped);
    assert!(!state
        .native_delivery
        .has_agent_owner(RECIPIENT)
        .await
        .unwrap());
    drop(lifecycle);
    let old_pending_result = tokio::time::timeout(DEADLINE, pending_rpc)
        .await
        .unwrap()
        .unwrap();
    replacement = crate::commands::agent::prepare_clear_replacement_config(&replacement).unwrap();
    assert!(replacement.resume_session.is_none());
    assert!(replacement.fresh_provider_session_id.is_none());

    // Spawn owns this counter in production. Exercise that same state transition
    // after the clear boundary without bringing the Wry/archive wrapper into scope.
    let new_generation = state
        .interactions
        .start_provider_input_generation(RECIPIENT, ProviderInputReadiness::Ready, None)
        .await
        .generation;
    let new_directory = home.path().join("new-owner");
    std::fs::create_dir(&new_directory).unwrap();
    let mut new_owner = stop_fixture(
        &new_directory,
        RECIPIENT,
        new_generation,
        NEW_THREAD,
        WriterCase::Owned,
    )
    .await;
    let (new_client, new_peer) = reset_peer(new_generation, NEW_THREAD).await;
    new_owner.owner.client.close().await;
    let unpublished_owner =
        Arc::get_mut(&mut new_owner.owner).expect("fixture owns unpublished replacement");
    unpublished_owner.client = new_client;
    unpublished_owner.attachment.cleared_provider_sessions =
        replacement.codex_config().cleared_provider_sessions;
    state
        .native_delivery
        .install_stop_test_owner(RECIPIENT, new_generation, None, new_owner.owner.clone())
        .await;
    // The fixture supplies the contained protocol owner; normal broker preparation
    // still checks the registered generation and returns that exact attachment.
    let attachment = state
        .native_delivery
        .prepare_codex_tui(
            NativeSessionSpec {
                target_agent_id: RECIPIENT.into(),
                provider: "codex".into(),
                generation: new_generation,
                workspace: home.path().to_path_buf(),
                config: AgentConfig {
                    is_off: true,
                    ..replacement.clone()
                },
            },
            None,
            None,
        )
        .await
        .unwrap();
    replacement.resume_session = attachment.expected_resume_id;
    {
        let agents = state.agents.lock().await;
        *agents[RECIPIENT].config.lock().unwrap() = replacement;
        *agents[RECIPIENT].current_status.lock().unwrap() = "Idle".into();
    }
    let new_binding = state
        .native_delivery
        .codex_binding(RECIPIENT, new_generation)
        .await
        .unwrap();
    assert_ne!(new_generation, old_generation);
    assert_eq!(new_binding.provider_session_id, NEW_THREAD);
    assert_ne!(new_binding.provider_session_id, OLD_THREAD);
    let late_old_write = old_client
        .followup(&delivered.record.id, &old_context)
        .await;
    let stale_owner_write = state
        .native_delivery
        .codex_followup(
            RECIPIENT,
            old_generation,
            &delivered.record.id,
            &old_context,
        )
        .await;

    let task = admit_reset_message(&state, new_generation, NEW_TASK, true).await;
    let dispatch =
        super::super::agent_messaging::dispatch_pending_queue(None, &state, RECIPIENT).await;
    let writes: Vec<_> = new_peer
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| {
            matches!(
                request["method"].as_str(),
                Some("turn/start" | "turn/steer" | "thread/inject_items")
            )
        })
        .cloned()
        .collect();
    let settled_reply = state
        .interactions
        .structured_reply(&settled.record.id)
        .await
        .unwrap();

    // Join owned processes and readers before checking the reset contract.
    state
        .native_delivery
        .dispose_agent(RECIPIENT)
        .await
        .unwrap();
    old_owner.cleanup().await;
    new_owner.cleanup().await;
    old_peer.finish().await;
    new_peer.finish().await;
    tokio::time::timeout(DEADLINE, async {
        while !store::with_db(store::pending_task_turn_bindings)
            .unwrap()
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("completion observer must retire before the isolated home is restored");

    dispatch.expect("the first new task reaches native dispatch");
    assert!(
        old_pending_result.is_err(),
        "reset must close an RPC waiting on the old owner"
    );
    assert!(
        late_old_write.is_err(),
        "a retained old client must not write after reset"
    );
    assert!(
        stale_owner_write.is_err(),
        "old generation cannot select the replacement owner"
    );
    assert_eq!(
        settled_reply.status,
        ReplyStatus::Done,
        "explicit correlated replies remain terminal"
    );
    let tasks: Vec<_> = writes
        .iter()
        .filter(|request| request["method"] == "turn/start")
        .collect();
    assert_eq!(
        tasks.len(),
        1,
        "exactly the new task {} must start after reset",
        task.record.id
    );
    assert_eq!(tasks[0]["params"]["threadId"], NEW_THREAD);
    let wire = serde_json::to_string(&writes).unwrap();
    for old in [OLD_PENDING, OLD_DELIVERED, OLD_SETTLED] {
        assert!(
            !wire.contains(old),
            "RESET leaked old conversation context into the first new task: {old}"
        );
    }
    assert!(
        wire.contains("only the new task"),
        "the new task must actually reach the provider"
    );
}
