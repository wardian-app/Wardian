//! Owned local protocol peers exercise framing and the continuous reader. They
//! neither launch Codex nor stand in for the separate real-provider acceptance.
use super::*;
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;

type Peer = WebSocketStream<TcpStream>;

fn frame() -> String {
    json!({
        "schema_version":1, "sender":"peer-sender", "recipient":"wardian-id",
        "kind":"task", "interaction_id":"task-id", "request_id":"task-id",
        "parent_interaction_id":"parent-id", "reply_status":null,
        "host_automation":{"run_id":"run-id","node":"node-id"},
        "body":"  literal peer body\nλ中 \"quotes\"\\\t  "
    })
    .to_string()
}

async fn connected(version: &str, status: Value) -> (Arc<CodexSharedClient>, Peer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let (client, socket) = tokio::join!(
        CodexSharedClient::connect("wardian-id".into(), 7, &endpoint, "owned-token"),
        async {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream).await.unwrap()
        }
    );
    let client = client.unwrap();
    client
        .bind(&json!({"thread":{"id":"owned","canAcceptDirectInput":true,"status":status}}))
        .unwrap();
    client
        .observation
        .send_modify(|state| state.provider_version = Some(version.into()));
    (client, socket)
}

async fn receive(socket: &mut Peer) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

async fn send(socket: &mut Peer, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

async fn assert_no_write(socket: &mut Peer) {
    assert!(
        tokio::time::timeout(Duration::from_millis(25), socket.next())
            .await
            .is_err()
    );
}

fn active(client: &CodexSharedClient) {
    client.observation.send_modify(|state| {
        state.observe(&json!({
            "method":"turn/started","params":{"threadId":"owned","turn":{"id":"active"}}
        }))
    });
}

#[tokio::test]
async fn exact_active_steer_preserves_body_and_host_only_routing() {
    let (client, mut socket) = connected("0.159.2", json!({"type":"active"})).await;
    active(&client);
    let context = frame();
    let expected: Value = serde_json::from_str(&context).unwrap();
    let admitted = client.followup("message-id", &context);
    let provider = async {
        let request = receive(&mut socket).await;
        assert_eq!(request["method"], "turn/steer");
        let params = &request["params"];
        assert_eq!(params["threadId"], "owned");
        assert_eq!(params["expectedTurnId"], "active");
        assert_eq!(
            params["input"],
            json!([{"type":"text","text":expected["body"]}])
        );
        assert!(params.get("toolOutput").is_none());
        let contexts = params["additionalContext"].as_object().unwrap();
        assert_eq!(contexts.len(), 1);
        let (key, context) = contexts.iter().next().unwrap();
        assert!(uuid::Uuid::parse_str(key).is_ok());
        assert_eq!(context["kind"], "application");
        let routing: Value = serde_json::from_str(context["value"].as_str().unwrap()).unwrap();
        let mut expected_routing = expected.clone();
        expected_routing.as_object_mut().unwrap().remove("body");
        expected_routing["task_outcome_instructions"] = json!(TASK_OUTCOME_INSTRUCTIONS);
        assert_eq!(routing, expected_routing);
        assert!(!context.to_string().contains("literal peer body"));
        // Completion before acknowledgement must remain available on this client.
        send(
            &mut socket,
            json!({"method":"item/agentMessage/delta","params":{
                "threadId":"owned","turnId":"active","itemId":"final","delta":"streamed commentary"
            }}),
        )
        .await;
        send(&mut socket, json!({"method":"item/completed","params":{
            "threadId":"foreign","turnId":"active","item":{"id":"foreign","type":"agentMessage","phase":"final_answer","text":"foreign answer"}
        }})).await;
        send(&mut socket, json!({"method":"item/completed","params":{
            "threadId":"owned","turnId":"other","item":{"id":"other","type":"agentMessage","phase":"final_answer","text":"other answer"}
        }})).await;
        send(&mut socket, json!({"method":"item/completed","params":{
            "threadId":"owned","turnId":"active","item":{"id":"final","type":"agentMessage","phase":"final_answer","text":"authoritative answer"}
        }})).await;
        send(
            &mut socket,
            json!({"method":"turn/completed","params":{
                "threadId":"owned","turn":{"id":"active","status":"completed"}
            }}),
        )
        .await;
        send(
            &mut socket,
            json!({"id":request["id"],"result":{"turnId":"active"}}),
        )
        .await;
    };
    let (receipt, ()) = tokio::join!(admitted, provider);
    let receipt = receipt.unwrap();
    assert_eq!(receipt.admission_mode.as_deref(), Some("steer"));
    assert_eq!(receipt.provider_turn_id.as_deref(), Some("active"));
    assert_eq!(receipt.message_id.as_deref(), Some("message-id"));
    assert_eq!(
        client
            .wait_for_final_result("active", Duration::from_secs(1))
            .await
            .unwrap(),
        ("completed".into(), "authoritative answer".into())
    );
    assert_no_write(&mut socket).await;
    client.close().await;
}

#[tokio::test]
async fn idle_start_keeps_tool_output_for_older_versions_and_cached_terminal_status() {
    for (version, terminal) in [
        ("0.154.0", "completed"),
        ("0.154.0-alpha.6", "interrupted"),
        ("0.159.1", "failed"),
        ("0.159.2", "completed"),
    ] {
        let (client, mut socket) = connected(version, json!({"type":"idle"})).await;
        let context = frame();
        let provider = async {
            let request = receive(&mut socket).await;
            assert_eq!(request["method"], "turn/start");
            let contexts = request["params"]["additionalContext"].as_object().unwrap();
            assert_eq!(contexts.len(), 1);
            let host_context = contexts.values().next().unwrap();
            assert_eq!(host_context["kind"], "application");
            assert_eq!(host_context["value"], TASK_OUTCOME_INSTRUCTIONS);
            let mut original_params = request["params"].clone();
            original_params
                .as_object_mut()
                .unwrap()
                .remove("additionalContext");
            assert_eq!(
                original_params,
                json!({"threadId":"owned","input":[],"toolOutput":{
                    "name":"wardian_task_delivery","namespace":"wardian","output":context
                }})
            );
            // A completed item and turn are sufficient exact evidence even if
            // turn/started has not arrived before the acknowledgement.
            send(&mut socket, json!({"method":"item/completed","params":{
                "threadId":"owned","turnId":"new","item":{"id":"answer","type":"agentMessage","text":"legacy final"}
            }})).await;
            send(
                &mut socket,
                json!({"method":"turn/completed","params":{
                    "threadId":"owned","turn":{"id":"new","status":terminal}
                }}),
            )
            .await;
            send(
                &mut socket,
                json!({"id":request["id"],"result":{"turn":{"id":"new"}}}),
            )
            .await;
        };
        let (receipt, ()) = tokio::join!(client.followup("message-id", &context), provider);
        assert_eq!(receipt.unwrap().admission_mode.as_deref(), Some("start"));
        assert_eq!(
            client
                .wait_for_final_result("new", Duration::from_secs(1))
                .await
                .unwrap(),
            (terminal.into(), "legacy final".into())
        );
        client.close().await;
    }
}

#[tokio::test]
async fn pending_missing_turn_old_active_and_malformed_frames_fail_before_write() {
    for (version, status, has_turn) in [
        ("0.159.2", Value::Null, false),
        ("0.159.2", json!({"type":"active"}), false),
        (
            "0.159.2",
            json!({"type":"active","activeFlags":["waitingOnApproval"]}),
            false,
        ),
        ("0.154.0", json!({"type":"active"}), true),
        ("0.154.0-alpha.6", json!({"type":"active"}), true),
        ("0.159.1", json!({"type":"active"}), true),
        ("0.159.2-alpha.1", json!({"type":"active"}), true),
    ] {
        let (client, mut socket) = connected(version, status).await;
        if has_turn {
            active(&client);
        }
        let error = client.followup("task-id", &frame()).await.unwrap_err();
        assert!(!error.provider_boundary_crossed, "{version}");
        assert_eq!(error.code, "unsupported");
        assert_no_write(&mut socket).await;
        client.close().await;
    }
    let (client, mut socket) = connected("0.159.2", json!({"type":"idle"})).await;
    for context in ["not json", "{}"] {
        assert!(
            !client
                .followup("task-id", context)
                .await
                .unwrap_err()
                .provider_boundary_crossed
        );
    }
    assert_no_write(&mut socket).await;
    client.close().await;
}

#[tokio::test]
async fn rejected_mismatched_and_disconnected_steer_never_fall_back_or_replay() {
    for response in [
        json!({"result":{"turnId":"wrong"}}),
        json!({"result":{}}),
        json!({"error":{"code":-32000,"message":"turn changed"}}),
        Value::Null,
    ] {
        let (client, mut socket) = connected("0.159.2", json!({"type":"active"})).await;
        active(&client);
        let context = frame();
        let provider = async {
            let request = receive(&mut socket).await;
            assert_eq!(request["method"], "turn/steer");
            if response.is_null() {
                socket.close(None).await.unwrap();
            } else {
                let mut reply = response.clone();
                reply["id"] = request["id"].clone();
                send(&mut socket, reply).await;
            }
        };
        let (result, ()) = tokio::join!(client.followup("task-id", &context), provider);
        let error = result.unwrap_err();
        assert!(error.provider_boundary_crossed);
        assert_eq!(
            error.code,
            if response.get("error").is_some() {
                "provider_rejected"
            } else {
                "submitted_unconfirmed"
            }
        );
        if !response.is_null() {
            assert_no_write(&mut socket).await;
        }
        client.close().await;
    }
}

#[tokio::test]
async fn proven_stale_steer_rejections_fail_before_submit_without_another_write() {
    for (message, rejected_before_submit) in [
        ("expected active turn id `active` but found `other`", true),
        ("no active turn to steer", true),
        ("other invalid request", false),
        (
            "expected active turn id `different` but found `other`",
            false,
        ),
        ("no active turn to steer ", false),
    ] {
        let (client, mut socket) = connected("0.159.2", json!({"type":"active"})).await;
        active(&client);
        let context = frame();
        let provider = async {
            let request = receive(&mut socket).await;
            assert_eq!(request["method"], "turn/steer");
            assert_eq!(request["params"]["expectedTurnId"], "active");
            send(
                &mut socket,
                json!({"id":request["id"],"error":{
                    "code":-32600,"message":message
                }}),
            )
            .await;
        };
        let (result, ()) = tokio::join!(client.followup("task-id", &context), provider);
        let error = result.unwrap_err();
        assert_eq!(error.provider_boundary_crossed, !rejected_before_submit);
        assert_eq!(
            error.code,
            if rejected_before_submit {
                "stale_turn_rejected"
            } else {
                "provider_rejected"
            }
        );
        assert_eq!(
            client.observation.borrow().activity(),
            CodexTurnActivity::Processing("active".into())
        );
        assert!(client.pending.lock().unwrap().is_empty());
        // The core caller releases the original claim. Only a later, separately
        // observed activity opportunity may authorize a new admission attempt.
        assert_no_write(&mut socket).await;
        client.close().await;
    }
}

#[tokio::test]
async fn malformed_mixed_result_and_stale_error_remains_uncertain() {
    let (client, mut socket) = connected("0.159.2", json!({"type":"active"})).await;
    active(&client);
    let context = frame();
    let provider = async {
        let request = receive(&mut socket).await;
        send(
            &mut socket,
            json!({"id":request["id"],"result":{"turnId":"active"},
                "error":{"code":-32600,"message":"no active turn to steer"}
            }),
        )
        .await;
    };
    let (result, ()) = tokio::join!(client.followup("task-id", &context), provider);
    let error = result.unwrap_err();
    assert!(error.provider_boundary_crossed);
    assert_eq!(error.code, "provider_rejected");
    assert_no_write(&mut socket).await;
    client.close().await;
}

#[tokio::test]
async fn activity_race_while_waiting_for_writer_fails_without_writing() {
    let (client, mut socket) = connected("0.159.2", json!({"type":"active"})).await;
    active(&client);
    let writer = client.writer.lock().await;
    let submit_client = client.clone();
    let submitted = tokio::spawn(async move { submit_client.followup("task-id", &frame()).await });
    // Wait until this request is registered and blocked at the writer fence.
    tokio::time::timeout(Duration::from_secs(1), async {
        while client.pending.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.observation.send_modify(|state| state.observe(&json!({
        "method":"turn/completed","params":{"threadId":"owned","turn":{"id":"active","status":"completed"}}
    })));
    drop(writer);
    let error = submitted.await.unwrap().unwrap_err();
    assert!(!error.provider_boundary_crossed);
    assert_eq!(error.code, "task_activity_deferred");
    assert!(client.pending.lock().unwrap().is_empty());
    assert_no_write(&mut socket).await;
    client.close().await;
}

#[tokio::test]
async fn information_and_replies_inject_without_waking_old_or_pending_owners() {
    for (version, status) in [
        ("0.154.0", json!({"type":"idle"})),
        ("0.154.0-alpha.6", json!({"type":"active"})),
        ("0.159.2", Value::Null),
    ] {
        let (client, mut socket) = connected(version, status).await;
        for kind in ["message", "reply"] {
            let mut context: Value = serde_json::from_str(&frame()).unwrap();
            context["kind"] = json!(kind);
            let provider = async {
                let request = receive(&mut socket).await;
                assert_eq!(request["method"], "thread/inject_items");
                assert_eq!(request["params"]["items"][0]["output"], context.to_string());
                send(&mut socket, json!({"id":request["id"],"result":{}})).await;
            };
            let (result, ()) = tokio::join!(client.push(context.clone()), provider);
            let receipt = result.unwrap();
            assert!(receipt.admission_mode.is_none());
            assert!(receipt.provider_turn_id.is_none());
            assert_no_write(&mut socket).await;
        }
        client.close().await;
    }
}

#[tokio::test]
async fn exact_wait_disconnect_and_timeout_remain_uncertain() {
    let (client, mut socket) = connected("0.159.2", json!({"type":"active"})).await;
    active(&client);
    let timeout = client
        .wait_for_final_result("active", Duration::from_millis(10))
        .await
        .unwrap_err();
    assert_eq!(timeout.code, "submitted_unconfirmed");
    assert!(timeout.provider_boundary_crossed);
    socket.close(None).await.unwrap();
    let disconnected = client
        .wait_for_final_result("active", Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(disconnected.code, "submitted_unconfirmed");
    assert!(disconnected.provider_boundary_crossed);
    client.close().await;
}

#[tokio::test]
async fn compaction_notifications_do_not_write_empty_steer_or_start_a_turn() {
    let (client, mut socket) = connected("0.160.0", json!({"type":"active"})).await;
    active(&client);
    let mut observations = client.observations();
    send(
        &mut socket,
        json!({"method":"thread/compacted","params":{"threadId":"owned","turnId":"active"}}),
    )
    .await;
    send(&mut socket, json!({"method":"item/completed","params":{"threadId":"owned","turnId":"active","item":{"type":"contextCompaction","id":"compaction"}}})).await;
    tokio::time::timeout(Duration::from_secs(1), observations.changed())
        .await
        .unwrap()
        .unwrap();
    assert_no_write(&mut socket).await;
    assert!(client.pending.lock().unwrap().is_empty());
    assert_eq!(
        client.observation.borrow().active_turn.as_deref(),
        Some("active")
    );
    client.close().await;
}
