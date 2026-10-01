//! Canonical DB -> exact local Codex protocol -> coordinator -> waiting receiver.
//! These tests own a loopback peer, not a provider process or a paid/native probe.

use super::*;
use crate::control::agent_messaging::{handle_in_state, Request, Response};
use crate::control::test_support::TestWardianHome;
use futures_util::{FutureExt, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::future::{poll_fn, Future};
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::task::Poll;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use wardian_core::agent_messaging::{AgentMessagePage, TaskDeliveryOwner};
use wardian_core::control::{InteractionKind, InteractionStatus, ProviderInputReadiness};

const REQUESTER: &str = "completion-requester";
const RECIPIENT: &str = "completion-recipient";
const THREAD: &str = "owned-thread";
const FINAL: &str = "\r\n final café 日本語 'quoted' `$() \\ \r\n";
const WAIT: Duration = Duration::from_secs(60);
const DEADLINE: Duration = Duration::from_secs(10);

struct Peer {
    socket: WebSocketStream<TcpStream>,
    task_starts: usize,
}

impl Peer {
    async fn read(&mut self) -> Value {
        let frame = self
            .socket
            .next()
            .await
            .expect("peer closed")
            .expect("peer read");
        let value: Value = serde_json::from_str(frame.to_text().expect("JSON text frame"))
            .expect("valid provider request");
        if value["method"] == "turn/start" {
            self.task_starts += 1;
        }
        value
    }

    async fn send(&mut self, value: Value) {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .expect("peer write");
    }

    async fn completed_item(
        &mut self,
        thread: &str,
        turn: &str,
        id: &str,
        phase: &str,
        text: &str,
    ) {
        self.send(json!({"method":"item/completed","params":{
            "threadId":thread,"turnId":turn,
            "item":{"type":"agentMessage","id":id,"text":text,"phase":phase}
        }}))
        .await;
    }

    async fn complete(&mut self, thread: &str, turn: &str, text: &str) {
        self.completed_item(thread, turn, "answer", "final_answer", text)
            .await;
        self.send(json!({"method":"turn/completed","params":{
            "threadId":thread,"turn":{"id":turn,"status":"completed"}
        }}))
        .await;
    }

    /// A response on the same socket proves all preceding events reached the
    /// continuous reader. Unexpected RPCs fail here instead of relying on sleep.
    async fn barrier(&mut self, client: &CodexSharedClient) {
        let server = async {
            let request = self.read().await;
            assert_eq!(
                request["method"], "wardian-test/barrier",
                "unexpected provider side effect"
            );
            self.send(json!({"id":request["id"],"result":{}})).await;
        };
        let (result, ()) = tokio::join!(client.request("wardian-test/barrier", json!({})), server);
        result.expect("reader barrier acknowledgement");
    }
}

struct Fixture {
    state: AppState,
    client: Arc<CodexSharedClient>,
    peer: Peer,
    generation: u64,
}

impl Fixture {
    async fn new() -> Self {
        let state = AppState::new();
        for id in [REQUESTER, RECIPIENT] {
            let mut agent = crate::control::tests::test_agent(id, id, "Test");
            agent.process_id = None;
            agent.config.lock().unwrap().provider = "codex".into();
            *agent.current_status.lock().unwrap() = "Off".into();
            state.agents.lock().await.insert(id.into(), agent);
        }
        let generation = state
            .interactions
            .start_provider_input_generation(RECIPIENT, ProviderInputReadiness::Ready, None)
            .await
            .generation;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned loopback listener");
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let accept = async {
            let (stream, _) = listener.accept().await.unwrap();
            // Tungstenite's Callback fixes ErrorResponse's size; the fixture
            // cannot box it while implementing that external contract.
            #[allow(clippy::result_large_err)]
            let authenticate =
                |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                    assert_eq!(
                        request.headers()["Authorization"],
                        "Bearer completion-test-token"
                    );
                    Ok(response)
                };
            tokio_tungstenite::accept_hdr_async(stream, authenticate)
                .await
                .unwrap()
        };
        let (client, socket) = tokio::join!(
            CodexSharedClient::connect(
                RECIPIENT.into(),
                generation,
                &endpoint,
                "completion-test-token"
            ),
            accept,
        );
        let client = client.expect("connect exact generation-bound client");
        client
            .bind(&json!({"thread":{
                "id":THREAD,"canAcceptDirectInput":true,"status":{"type":"idle"},"turns":[]
            }}))
            .expect("positively idle owned thread");
        Self {
            state,
            client,
            peer: Peer {
                socket,
                task_starts: 0,
            },
            generation,
        }
    }

    async fn task(&mut self, turn: &str, early: bool) -> store::TaskTurnBinding {
        let response = handle_in_state(
            None,
            &self.state,
            Request::FollowupTask {
                target: RECIPIENT.into(),
                message: "literal task λ\r\n".into(),
                idempotency_key: None,
            },
            origin(REQUESTER),
        )
        .await
        .unwrap();
        let Response::FollowupTask {
            request_id,
            delivery_owner,
            ..
        } = response
        else {
            panic!("task admission response");
        };
        assert_eq!(delivery_owner, TaskDeliveryOwner::Unclaimed);
        let claim = self
            .state
            .interactions
            .claim_agent_task(RECIPIENT, self.generation)
            .await
            .unwrap()
            .expect("durable scheduler claim before provider write");
        assert_eq!(claim.record.id, request_id);
        let frame = store::with_db(|conn| {
            let record = store::load(conn, &request_id)?;
            assert_eq!(record.status, InteractionStatus::AwaitingReply);
            store::message_context(conn, &record)
        })
        .unwrap();
        let context = serde_json::to_string(&frame).unwrap();
        let provider = async {
            let request = self.peer.read().await;
            assert_eq!(request["method"], "turn/start");
            assert_eq!(request["params"]["threadId"], THREAD);
            assert_eq!(request["params"]["input"], json!([]));
            let written: Value =
                serde_json::from_str(request["params"]["toolOutput"]["output"].as_str().unwrap())
                    .unwrap();
            assert_eq!(written, serde_json::to_value(&frame).unwrap());
            assert_eq!(written["request_id"], request_id);
            if early {
                // Both events precede the RPC response in the reader's stream.
                self.peer.complete(THREAD, turn, FINAL).await;
            } else {
                self.peer
                    .send(json!({"method":"turn/started","params":{
                        "threadId":THREAD,"turn":{"id":turn}
                    }}))
                    .await;
            }
            self.peer
                .send(json!({"id":request["id"],"result":{"turn":{"id":turn}}}))
                .await;
        };
        let (receipt, ()) = tokio::join!(self.client.followup(&request_id, &context), provider);
        let receipt = receipt.expect("exact task acknowledgement");
        assert_eq!(receipt.admission_mode.as_deref(), Some("start"));
        let binding = bind_codex_task(&self.state, &claim, &receipt)
            .await
            .unwrap();
        assert_eq!(binding.request_id, request_id);
        assert_eq!(binding.recipient, RECIPIENT);
        assert_eq!(binding.generation, self.generation);
        assert_eq!(binding.provider_session_id, THREAD);
        assert_eq!(binding.provider_turn_id, turn);
        self.peer.barrier(&self.client).await;
        binding
    }

    async fn assert_no_process_start(&mut self, expected_tasks: usize) {
        self.peer.barrier(&self.client).await;
        assert_eq!(
            self.peer.task_starts, expected_tasks,
            "only explicitly admitted tasks start turns"
        );
        let agents = self.state.agents.lock().await;
        for agent in agents.values() {
            assert!(agent.child_process.is_none());
            assert!(agent.background_processes.is_empty());
            assert!(agent.process_id.is_none());
            assert!(agent.runtime_generation.is_none());
            assert_eq!(*agent.current_status.lock().unwrap(), "Off");
            assert_eq!(*agent.query_count.lock().unwrap(), 0);
        }
        drop(agents);
        assert_eq!(
            self.state
                .interactions
                .current_provider_input_generation(REQUESTER)
                .await,
            None
        );
    }
}

/// The home/global environment guard outlives cleanup, even on assertion panic
/// or external timeout. Borrowed observer/wait futures leave no detached workers.
async fn scenario(
    run: impl for<'a> FnOnce(&'a mut Fixture) -> Pin<Box<dyn Future<Output = ()> + 'a>>,
) {
    let _home = TestWardianHome::new_async().await;
    let mut fixture = tokio::time::timeout(DEADLINE, Fixture::new())
        .await
        .expect("fixture deadline");
    let outcome = AssertUnwindSafe(tokio::time::timeout(DEADLINE, run(&mut fixture)))
        .catch_unwind()
        .await;
    fixture.client.close().await;
    match outcome {
        Ok(result) => result.expect("integration scenario deadline"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn origin(id: &str) -> MessageOrigin {
    MessageOrigin::WardianAgent {
        session_id: id.into(),
    }
}

async fn receive(
    state: &AppState,
    recipient: &str,
    timeout_ms: u64,
    cursor: Option<String>,
    ack_cursor: Option<String>,
) -> Result<AgentMessagePage, ControlError> {
    let response = handle_in_state(
        None,
        state,
        Request::ReceiveMessages {
            cursor,
            ack_cursor,
            limit: None,
            timeout_ms: Some(timeout_ms),
        },
        origin(recipient),
    )
    .await?;
    let Response::ReceiveMessages { page } = response else {
        panic!("receive response")
    };
    Ok(page)
}

/// With uncontended fixture locks and synchronous DB reads, the first Pending
/// registers the actual mailbox/turn wait before the test publishes any event.
async fn assert_pending<F: Future>(mut future: Pin<&mut F>) {
    assert!(
        poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await,
        "expected a registered wait, not an already available response"
    );
}

fn assert_reply(page: &AgentMessagePage, request: &str, status: ReplyStatus, body: &str) {
    assert!(!page.timed_out);
    assert_eq!(
        page.messages.len(),
        1,
        "exactly one correlated canonical reply"
    );
    let reply = &page.messages[0];
    assert_eq!(reply.kind, InteractionKind::Reply);
    assert_eq!(reply.sender, RECIPIENT);
    assert_eq!(reply.parent_interaction_id.as_deref(), Some(request));
    assert_eq!(reply.reply_status, Some(status));
    assert_eq!(reply.message.as_bytes(), body.as_bytes());
    let count = store::with_db(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM structured_replies WHERE request_id=?1",
            [request],
            |row| row.get::<_, i64>(0),
        )?)
    })
    .unwrap();
    assert_eq!(
        count, 1,
        "one durable result, not just one projected message"
    );
}

#[tokio::test]
async fn automatic_final_wakes_concurrent_waiters_without_ack_or_timeout_settlement() {
    scenario(|fixture| {
        Box::pin(async move {
            let binding = fixture.task("bound", false).await;
            assert!(
                receive(&fixture.state, RECIPIENT, 0, None, None)
                    .await
                    .unwrap()
                    .messages
                    .is_empty(),
                "provider-owned task must not also reach receiver delivery"
            );
            let timed = receive(&fixture.state, REQUESTER, 1, None, None)
                .await
                .unwrap();
            assert!(timed.timed_out);
            assert!(timed.messages.is_empty());
            assert_eq!(
                store::with_db(|conn| store::load(conn, &binding.request_id))
                    .unwrap()
                    .status,
                InteractionStatus::AwaitingReply
            );
            assert_eq!(
                store::with_db(store::pending_task_turn_bindings).unwrap(),
                vec![binding.clone()]
            );
            {
                let first = receive(&fixture.state, REQUESTER, 60_000, None, None);
                let second = receive(&fixture.state, REQUESTER, 60_000, None, None);
                let observer = observe_codex_task(
                    &fixture.state.interactions,
                    &fixture.client,
                    &binding,
                    WAIT,
                );
                tokio::pin!(first, second, observer);
                assert_pending(first.as_mut()).await;
                assert_pending(second.as_mut()).await;
                assert_pending(observer.as_mut()).await;
                fixture
                    .peer
                    .complete("foreign-thread", "bound", "foreign answer")
                    .await;
                fixture
                    .peer
                    .complete(THREAD, "unrelated-turn", "unrelated answer")
                    .await;
                fixture.peer.send(json!({"method":"item/agentMessage/delta","params":{
                "threadId":THREAD,"turnId":"bound","itemId":"stream","delta":"streamed commentary"
            }})).await;
                fixture
                    .peer
                    .completed_item(THREAD, "bound", "commentary", "commentary", "thinking")
                    .await;
                fixture.peer.barrier(&fixture.client).await;
                assert_pending(observer.as_mut()).await;
                assert_pending(first.as_mut()).await;
                assert_pending(second.as_mut()).await;
                assert!(fixture
                    .state
                    .interactions
                    .structured_reply(&binding.request_id)
                    .await
                    .is_none());
                fixture.peer.complete(THREAD, "bound", FINAL).await;
                let (observed, first, second) = tokio::join!(observer, first, second);
                assert_eq!(observed.unwrap().1, FINAL);
                let first = first.unwrap();
                let second = second.unwrap();
                assert_reply(&first, &binding.request_id, ReplyStatus::Done, FINAL);
                assert_eq!(first.messages, second.messages);
                assert_eq!(first.next_cursor, second.next_cursor);
                assert_eq!(first.ack_cursor, second.ack_cursor);
                let replay = receive(&fixture.state, REQUESTER, 0, None, None)
                    .await
                    .unwrap();
                assert_eq!(
                    replay.messages, first.messages,
                    "waiters must not automatically ack"
                );
                let acked = receive(&fixture.state, REQUESTER, 0, None, Some(first.ack_cursor))
                    .await
                    .unwrap();
                assert!(acked.messages.is_empty());
            }
            let duplicate =
                observe_codex_task(&fixture.state.interactions, &fixture.client, &binding, WAIT)
                    .await
                    .unwrap();
            assert!(duplicate.0.is_none());
            assert_eq!(
                store::with_db(|conn| store::load(conn, &binding.request_id))
                    .unwrap()
                    .status,
                InteractionStatus::Completed
            );
            assert!(store::with_db(store::pending_task_turn_bindings)
                .unwrap()
                .is_empty());
            assert!(receive(&fixture.state, REQUESTER, 0, None, None)
                .await
                .unwrap()
                .messages
                .is_empty());
            fixture.assert_no_process_start(1).await;
        })
    })
    .await;
}

#[tokio::test]
async fn exact_completion_before_ack_is_available_to_late_coordinator_observer() {
    scenario(|fixture| {
        Box::pin(async move {
            let binding = fixture.task("early", true).await;
            {
                let wait = receive(&fixture.state, REQUESTER, 60_000, None, None);
                tokio::pin!(wait);
                assert_pending(wait.as_mut()).await;
                let observed = observe_codex_task(
                    &fixture.state.interactions,
                    &fixture.client,
                    &binding,
                    WAIT,
                );
                let (observed, page) = tokio::join!(observed, wait);
                assert_eq!(observed.unwrap().1, FINAL);
                assert_reply(
                    &page.unwrap(),
                    &binding.request_id,
                    ReplyStatus::Done,
                    FINAL,
                );
            }
            fixture.assert_no_process_start(1).await;
        })
    })
    .await;
}

#[tokio::test]
async fn explicit_done_failed_and_blocked_win_over_later_automatic_final() {
    scenario(|fixture| {
        Box::pin(async move {
            for (index, status) in [ReplyStatus::Done, ReplyStatus::Failed, ReplyStatus::Blocked]
                .into_iter()
                .enumerate()
            {
                let turn = format!("explicit-{index}");
                let binding = fixture.task(&turn, false).await;
                {
                    let wait = receive(&fixture.state, REQUESTER, 60_000, None, None);
                    let observer = observe_codex_task(
                        &fixture.state.interactions,
                        &fixture.client,
                        &binding,
                        WAIT,
                    );
                    tokio::pin!(wait, observer);
                    assert_pending(wait.as_mut()).await;
                    assert_pending(observer.as_mut()).await;
                    let explicit = handle_in_state(
                        None,
                        &fixture.state,
                        Request::Reply {
                            request_id: binding.request_id.clone(),
                            status: status.clone(),
                            message: "explicit result".into(),
                        },
                        origin(RECIPIENT),
                    )
                    .await
                    .unwrap();
                    let Response::Reply {
                        interaction_id,
                        duplicate,
                        ..
                    } = explicit
                    else {
                        panic!("reply response")
                    };
                    assert!(!duplicate);
                    let page = wait.await.unwrap();
                    assert_reply(
                        &page,
                        &binding.request_id,
                        status.clone(),
                        "explicit result",
                    );
                    assert_eq!(page.messages[0].interaction_id, interaction_id);
                    fixture.peer.complete(THREAD, &turn, FINAL).await;
                    let observed = observer.await.unwrap();
                    assert!(
                        observed.0.is_none(),
                        "explicit terminal reply must suppress automatic publication"
                    );
                    assert_eq!(observed.1, FINAL);
                    let replay = receive(&fixture.state, REQUESTER, 0, None, None)
                        .await
                        .unwrap();
                    assert_eq!(replay.messages, page.messages);
                    assert_reply(&replay, &binding.request_id, status, "explicit result");
                    receive(&fixture.state, REQUESTER, 0, None, Some(page.ack_cursor))
                        .await
                        .unwrap();
                }
            }
            fixture.assert_no_process_start(3).await;
        })
    })
    .await;
}

#[tokio::test]
async fn replacement_fences_late_final_and_deletion_wakes_and_rejects_waiter() {
    // Fresh acknowledged tasks keep deletion evidence independent of the
    // replacement fence. A surviving roster entry cannot authorize a deleted ID.
    for deleted in [None, Some(REQUESTER), Some(RECIPIENT)] {
        scenario(move |fixture| {
            Box::pin(async move {
                let binding = fixture.task("invalidated", false).await;
                {
                    let observer = observe_codex_task(
                        &fixture.state.interactions,
                        &fixture.client,
                        &binding,
                        WAIT,
                    );
                    let wait = receive(&fixture.state, REQUESTER, 60_000, None, None);
                    tokio::pin!(observer, wait);
                    assert_pending(observer.as_mut()).await;
                    assert_pending(wait.as_mut()).await;
                    if let Some(id) = deleted {
                        fixture
                            .state
                            .interactions
                            .delete_agent_durable_state(id)
                            .await
                            .unwrap();
                        if id == REQUESTER {
                            assert_eq!(wait.as_mut().await.unwrap_err().code, "unauthorized");
                        }
                        assert_eq!(
                            receive(&fixture.state, id, 0, None, None)
                                .await
                                .unwrap_err()
                                .code,
                            "unauthorized"
                        );
                        let sender = if id == REQUESTER {
                            RECIPIENT
                        } else {
                            REQUESTER
                        };
                        let admission = handle_in_state(
                            None,
                            &fixture.state,
                            Request::SendMessage {
                                target: id.into(),
                                message: "late info".into(),
                                idempotency_key: None,
                            },
                            origin(sender),
                        )
                        .await;
                        assert_eq!(admission.unwrap_err().code, "not_found");
                    } else {
                        let next = fixture
                            .state
                            .interactions
                            .start_provider_input_generation(
                                RECIPIENT,
                                ProviderInputReadiness::Ready,
                                None,
                            )
                            .await;
                        assert!(next.generation > binding.generation);
                        assert!(fixture
                            .state
                            .interactions
                            .claim_agent_task(RECIPIENT, next.generation)
                            .await
                            .unwrap()
                            .is_none());
                    }
                    fixture.peer.complete(THREAD, "invalidated", FINAL).await;
                    let error = observer
                        .await
                        .err()
                        .expect("invalidated task must reject late final");
                    if deleted != Some(REQUESTER) {
                        assert_eq!(error.code, "stale_claim");
                        assert_pending(wait.as_mut()).await;
                        assert!(receive(&fixture.state, REQUESTER, 0, None, None)
                            .await
                            .unwrap()
                            .messages
                            .is_empty());
                    }
                    if deleted.is_none() {
                        assert_eq!(
                            store::with_db(|conn| store::load(conn, &binding.request_id))
                                .unwrap()
                                .status,
                            InteractionStatus::AwaitingReply
                        );
                    }
                    assert!(fixture
                        .state
                        .interactions
                        .structured_reply(&binding.request_id)
                        .await
                        .is_none());
                    let replies = store::with_db(|conn| {
                        Ok(conn.query_row(
                            "SELECT COUNT(*) FROM structured_replies WHERE request_id=?1",
                            [&binding.request_id],
                            |row| row.get::<_, i64>(0),
                        )?)
                    })
                    .unwrap();
                    assert_eq!(replies, 0);
                }
                fixture.assert_no_process_start(1).await;
            })
        })
        .await;
    }
}

#[tokio::test]
async fn information_admission_wakes_existing_receive_without_starting_provider() {
    scenario(|fixture| {
        Box::pin(async move {
            {
                let wait = receive(&fixture.state, REQUESTER, 60_000, None, None);
                tokio::pin!(wait);
                assert_pending(wait.as_mut()).await;
                let response = handle_in_state(
                    None,
                    &fixture.state,
                    Request::SendMessage {
                        target: REQUESTER.into(),
                        message: FINAL.into(),
                        idempotency_key: None,
                    },
                    origin(RECIPIENT),
                )
                .await
                .unwrap();
                let Response::SendMessage { interaction_id, .. } = response else {
                    panic!("information response")
                };
                let page = wait.await.unwrap();
                assert!(!page.timed_out);
                assert!(
                    page.wake_reason.is_none(),
                    "ordinary information is not a provider-context wake"
                );
                assert_eq!(page.messages.len(), 1);
                assert_eq!(page.messages[0].interaction_id, interaction_id);
                assert_eq!(page.messages[0].kind, InteractionKind::Message);
                assert_eq!(page.messages[0].message.as_bytes(), FINAL.as_bytes());
                assert!(page.messages[0].parent_interaction_id.is_none());
                assert!(page.messages[0].reply_status.is_none());
                assert_eq!(
                    receive(&fixture.state, REQUESTER, 0, None, None)
                        .await
                        .unwrap()
                        .messages,
                    page.messages
                );
                assert!(fixture
                    .state
                    .interactions
                    .claim_agent_task(REQUESTER, 0)
                    .await
                    .unwrap()
                    .is_none());
            }
            fixture.assert_no_process_start(0).await;
        })
    })
    .await;
}

#[tokio::test]
async fn codex_deferral_rechecks_idle_after_release_when_normal_dispatch_was_consumed() {
    for failure_code in ["stale_turn_rejected", "task_activity_deferred"] {
        scenario(move |fixture| {
        Box::pin(async move {
            use crate::delivery::codex_shared::{CodexSharedError, CodexTurnActivity};
            use crate::delivery::native_broker::codex_shared_error_for_test;
            use wardian_core::native_transport::NativeDeliveryErrorCode;

            let task = fixture.state.interactions.admit_agent_message(store::Admission {
                sender: REQUESTER,
                recipient: RECIPIENT,
                message: "deferred task T",
                idempotency_key: None,
                task: true,
                generation: fixture.generation,
            }).await.unwrap();
            let claim = fixture.state.interactions.claim_agent_task(RECIPIENT, fixture.generation)
                .await.unwrap().expect("T is claimed before the deferral");
            assert_eq!(claim.record.id, task.record.id);
            assert!(store::with_db(|conn| store::owns_claim(conn, &claim)).unwrap());
            assert!(store::with_db(store::pending_task_turn_bindings).unwrap().is_empty());
            let unrelated = fixture.state.interactions.admit_agent_message(store::Admission {
                sender: REQUESTER,
                recipient: RECIPIENT,
                message: "unrelated task U",
                idempotency_key: None,
                task: true,
                generation: fixture.generation,
            }).await.unwrap();

            fixture.peer.send(json!({"method":"turn/started","params":{
                "threadId":THREAD,"turn":{"id":"A"}
            }})).await;
            fixture.peer.barrier(&fixture.client).await;
            let attempted = fixture.client.observations().borrow().activity();
            assert_eq!(attempted, CodexTurnActivity::Processing("A".into()));
            let rejection = codex_shared_error_for_test(CodexSharedError {
                code: failure_code.into(),
                message: format!("{failure_code}: positive task admission deferral"),
                provider_boundary_crossed: false,
            });
            assert_eq!(rejection.code, NativeDeliveryErrorCode::FailedBeforeSubmit,
                "{failure_code} must map to definite no-submit deferral");
            assert!(!rejection.provider_boundary_crossed);

            {
                // U commits its reply, then waits on the cache while retaining
                // mutation_lock. This holds T's claim release at a known boundary.
                let records = fixture.state.interactions.records.lock().await;
                let reply = handle_in_state(None, &fixture.state, Request::Reply {
                    request_id: unrelated.record.id.clone(),
                    status: ReplyStatus::Done,
                    message: "unrelated explicit result".into(),
                }, origin(RECIPIENT));
                tokio::pin!(reply);
                assert_pending(reply.as_mut()).await;
                let committed = store::with_db(|conn| Ok(conn.query_row(
                    "SELECT COUNT(*) FROM structured_replies WHERE request_id=?1",
                    [&unrelated.record.id], |row| row.get::<_, i64>(0),
                )?)).unwrap();
                assert_eq!(committed, 1, "U committed before blocking on records");

                let deferred = super::super::settle_codex_deferral(
                    &fixture.state, &claim, &fixture.client, &attempted, &rejection,
                );
                tokio::pin!(deferred);
                assert_pending(deferred.as_mut()).await;
                assert!(store::with_db(|conn| store::owns_claim(conn, &claim)).unwrap());

                fixture.peer.complete(THREAD, "A", FINAL).await;
                fixture.peer.barrier(&fixture.client).await;
                assert_eq!(fixture.client.observations().borrow().activity(),
                    CodexTurnActivity::Idle("A".into()));
                // Simulate the ordinary idle dispatch opportunity while T is
                // still dispatching. U is terminal, so no task can be claimed.
                assert!(store::with_db(|conn| store::next_pending_task_id(conn, RECIPIENT))
                    .unwrap().is_none(), "the normal idle opportunity finds no pending task");
                assert!(store::with_db(|conn| store::owns_claim(conn, &claim)).unwrap());

                drop(records);
                reply.await.expect("U releases mutation_lock after cache publication");
                let dispatch_opportunity = deferred.await.expect("T deferral settles");
                assert_eq!(store::with_db(|conn| store::next_pending_task_id(conn, RECIPIENT))
                    .unwrap().as_deref(), Some(task.record.id.as_str()));
                assert!(!store::with_db(|conn| store::owns_claim(conn, &claim)).unwrap());
                assert_eq!(store::with_db(|conn| store::load(conn, &task.record.id)).unwrap().status,
                    InteractionStatus::AwaitingReply);
                assert!(dispatch_opportunity,
                    "claim release must return a fresh dispatch opportunity after the consumed idle event");

                // Consume U's explicit result so the next receive proves T's
                // automatic result rather than replaying unrelated availability.
                let unrelated_page = receive(&fixture.state, REQUESTER, 0, None, None).await.unwrap();
                assert_reply(&unrelated_page, &unrelated.record.id, ReplyStatus::Done, "unrelated explicit result");
                receive(&fixture.state, REQUESTER, 0, None, Some(unrelated_page.ack_cursor)).await.unwrap();

                let released = fixture.state.interactions.claim_agent_task(RECIPIENT, fixture.generation)
                    .await.unwrap().expect("consume the returned dispatch opportunity for T");
                assert_eq!(released.record.id, task.record.id, "retry the same canonical task");
                assert_ne!(released.token, claim.token, "release requires a fresh dispatch claim");
                let frame = store::with_db(|conn| store::message_context(conn, &released.record)).unwrap();
                let context = serde_json::to_string(&frame).unwrap();
                let wait = receive(&fixture.state, REQUESTER, 60_000, None, None);
                tokio::pin!(wait);
                assert_pending(wait.as_mut()).await;
                assert_eq!(fixture.client.observations().borrow().activity(),
                    CodexTurnActivity::Idle("A".into()));

                // No provider event, prompt, or generation transition occurs
                // between release and submission. Only T's one RPC is written.
                let admission = async {
                    let request = fixture.peer.read().await;
                    assert_eq!(request["method"], "turn/start");
                    assert_eq!(request["params"]["threadId"], THREAD);
                    assert_eq!(request["params"]["input"], json!([]));
                    let written: Value = serde_json::from_str(
                        request["params"]["toolOutput"]["output"].as_str().unwrap(),
                    ).unwrap();
                    assert_eq!(written, serde_json::to_value(&frame).unwrap());
                    assert_eq!(written["request_id"], task.record.id);
                    assert!(store::with_db(|conn| store::owns_claim(conn, &released)).unwrap());
                    fixture.peer.send(json!({"id":request["id"],"result":{"turn":{"id":"T-retry"}}})).await;
                };
                let (receipt, ()) = tokio::join!(
                    fixture.client.followup(&task.record.id, &context), admission,
                );
                let receipt = receipt.expect("one acknowledged admission of released T");
                assert_eq!(receipt.admission_mode.as_deref(), Some("start"));
                let binding = bind_codex_task(&fixture.state, &released, &receipt).await.unwrap();
                assert_eq!(binding.request_id, task.record.id);
                assert_eq!(binding.provider_turn_id, "T-retry");
                assert_eq!(binding.generation, fixture.generation);
                let observer = observe_codex_task(&fixture.state.interactions, &fixture.client, &binding, WAIT);
                tokio::pin!(observer);
                assert_pending(observer.as_mut()).await;
                assert_pending(wait.as_mut()).await;
                fixture.peer.complete(THREAD, "T-retry", FINAL).await;
                let (observed, page) = tokio::join!(observer, wait);
                let (published, answer) = observed.unwrap();
                assert_eq!(answer, FINAL);
                let published = published.expect("released T publishes one automatic result");
                let page = page.unwrap();
                assert_reply(&page, &task.record.id, ReplyStatus::Done, FINAL);
                assert_eq!(page.messages[0].interaction_id, published.record.id);
                assert_eq!(store::with_db(|conn| store::load(conn, &task.record.id)).unwrap().status,
                    InteractionStatus::Completed);
                assert!(store::with_db(store::pending_task_turn_bindings).unwrap().is_empty());
                assert!(fixture.state.interactions.claim_agent_task(RECIPIENT, fixture.generation)
                    .await.unwrap().is_none());
                let duplicate = observe_codex_task(&fixture.state.interactions, &fixture.client, &binding, WAIT)
                    .await.unwrap();
                assert!(duplicate.0.is_none(), "the admission produces only one canonical result");
                assert_eq!(receive(&fixture.state, REQUESTER, 0, None, None).await.unwrap().messages,
                    page.messages);
            }
            fixture.assert_no_process_start(1).await;
        })
    }).await;
    }
}
