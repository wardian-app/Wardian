//! Owned local protocol fixtures prove call/event correlation, not model recovery.
use super::*;
use std::future::Future;
use std::task::Poll;

async fn connected() -> (
    Arc<CodexSharedClient>,
    WebSocketStream<tokio::net::TcpStream>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let (client, peer) = tokio::join!(
        CodexSharedClient::connect("agent".into(), 7, &endpoint, "owned-token"),
        async {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream).await.unwrap()
        }
    );
    let client = client.unwrap();
    client.bind(&json!({"thread":{"id":"thread","canAcceptDirectInput":true,"status":{"type":"active"}}})).unwrap();
    client.observation.send_modify(|state| {
        state.provider_version = Some("0.160.0".into());
        state.task_context_policy = Some(super::super::task_context_policy::TaskContextPolicy {
            thread_id: "thread".into(),
            configuration_version: "qualified-fixture-version".into(),
        });
        state.observe(
            &json!({"method":"turn/started","params":{"threadId":"thread","turn":{"id":"A"}}}),
        );
    });
    (client, peer)
}

fn call() -> TaskContextCall {
    TaskContextCall {
        call_id: "native-call".into(),
        thread_id: "thread".into(),
        reported_session_id: "root-shared-session".into(),
        originating_item_id: Some("code-mode-cell".into()),
        window_id: Some("window".into()),
    }
}

fn event(method: &str, turn: &str, id: &str) -> Value {
    json!({"method":method,"params":{"threadId":"thread","turnId":turn,"item":{"type":"mcpToolCall","id":id,"server":"wardian","tool":"read_task_context"}}})
}

fn observe(client: &CodexSharedClient, value: Value) {
    client
        .observation
        .send_modify(|state| state.observe(&value));
}

#[tokio::test]
async fn exact_native_call_accepts_distinct_root_session_and_code_mode_origin() {
    let (client, mut peer) = connected().await;
    observe(&client, event("item/started", "A", "native-call"));
    let binding = client.task_context_binding(&call()).await.unwrap();
    assert_eq!(
        (
            binding.agent_id.as_str(),
            binding.generation,
            binding.thread_id.as_str(),
            binding.turn_id.as_str()
        ),
        ("agent", 7, "thread", "A")
    );
    assert_ne!(
        binding.provider_call.call_id,
        binding
            .provider_call
            .originating_item_id
            .as_ref()
            .unwrap()
            .as_str()
    );
    assert_ne!(binding.thread_id, binding.provider_call.reported_session_id);
    assert!(client.pending.lock().unwrap().is_empty());
    assert!(tokio::time::timeout(Duration::from_millis(25), peer.next())
        .await
        .is_err());
    client.close().await;
}

#[tokio::test]
async fn observable_reload_retires_policy_and_ready_does_not_revive_it() {
    let (client, _peer) = connected().await;
    observe(&client, event("item/started", "A", "native-call"));
    let binding = client.task_context_binding(&call()).await.unwrap();
    observe(
        &client,
        json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"thread","name":"wardian","status":"starting"}}),
    );
    observe(
        &client,
        json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"thread","name":"wardian","status":"ready"}}),
    );
    assert_eq!(
        client
            .validate_task_context_binding(&binding)
            .unwrap_err()
            .code,
        "task_context_unsupported"
    );
    client.close().await;
}

#[tokio::test]
async fn control_before_event_waits_for_same_call_without_resending() {
    let (client, _peer) = connected().await;
    let metadata = call();
    let mut read = Box::pin(client.task_context_binding(&metadata));
    std::future::poll_fn(|cx| {
        assert!(read.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    observe(&client, event("item/started", "A", "native-call"));
    assert_eq!(read.await.unwrap().turn_id, "A");
    assert!(client.pending.lock().unwrap().is_empty());
    client.close().await;
}

#[tokio::test]
async fn completed_call_replacement_turn_generation_and_close_reject_old_snapshot() {
    let (client, _peer) = connected().await;
    observe(&client, event("item/started", "A", "native-call"));
    let binding = client.task_context_binding(&call()).await.unwrap();
    let mut wrong_generation = binding.clone();
    wrong_generation.generation += 1;
    assert_eq!(
        client
            .validate_task_context_binding(&wrong_generation)
            .unwrap_err()
            .code,
        "stale_task_context"
    );
    observe(&client, event("item/completed", "A", "native-call"));
    assert_eq!(
        client
            .validate_task_context_binding(&binding)
            .unwrap_err()
            .code,
        "stale_task_context"
    );
    observe(
        &client,
        json!({"method":"turn/started","params":{"threadId":"thread","turn":{"id":"B"}}}),
    );
    observe(&client, event("item/started", "B", "B-call"));
    assert!(client.task_context_binding_now(&call()).unwrap().is_none());
    assert_eq!(
        client
            .validate_task_context_binding(&binding)
            .unwrap_err()
            .code,
        "stale_task_context"
    );
    client.close().await;
    assert_eq!(
        client.task_context_binding(&call()).await.unwrap_err().code,
        "stale_task_context"
    );
}

#[tokio::test]
async fn forged_wrong_tool_wrong_thread_duplicate_and_overflow_fail_closed() {
    let (client, _peer) = connected().await;
    for key in ["server", "tool"] {
        let mut wrong = event("item/started", "A", "native-call");
        wrong["params"]["item"][key] = json!("other");
        observe(&client, wrong);
    }
    let mut wrong_thread = event("item/started", "A", "native-call");
    wrong_thread["params"]["threadId"] = json!("other");
    observe(&client, wrong_thread);
    assert!(client.task_context_binding_now(&call()).unwrap().is_none());
    observe(&client, event("item/started", "A", "native-call"));
    observe(&client, event("item/started", "A", "native-call"));
    assert_eq!(
        client.task_context_binding(&call()).await.unwrap_err().code,
        "ambiguous_task_context"
    );
    observe(
        &client,
        json!({"method":"turn/started","params":{"threadId":"thread","turn":{"id":"B"}}}),
    );
    for index in 0..=MAX_OBSERVED_CALLS {
        observe(
            &client,
            event("item/started", "B", &format!("call-{index}")),
        );
    }
    assert_eq!(
        client.task_context_binding(&call()).await.unwrap_err().code,
        "ambiguous_task_context"
    );
    client.close().await;
}
