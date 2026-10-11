//! Wardian protocol regressions; synthetic transport is not stock persistence proof.
use super::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

async fn prepare_saved_resume(
    client: &CodexSharedClient,
    policy: &ExpectedPolicy,
    id: &str,
    alive: &mut impl FnMut() -> Result<(), CodexSharedError>,
) -> Result<(), CodexSharedError> {
    super::prepare_saved_resume(
        client,
        policy,
        id,
        alive,
        tokio::time::Instant::now() + STARTUP_TIMEOUT,
        "0.160.0",
    )
    .await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Auto,
    User,
    UserConflict,
    Busy,
    ForeignIdentity,
    CustomReviewer,
    MissingReviewer,
    WrongModel,
    WrongEffort,
    WrongApproval,
    WrongSandbox,
    NoDirectInput,
    InProgressTurn,
    ActiveNotification,
    TurnNotification,
    PrematureClosed,
    LaggedNotifications,
    PreFenceApplied,
    Disconnect,
    MissingApplied,
    ForeignApplied,
    ConflictingApplied,
    AsyncError,
    ReadbackMismatch,
    MissingClosed,
    ForeignClosed,
    StillLoaded,
    PersistenceLost,
    RetainedWriter,
}

fn policy() -> ExpectedPolicy {
    ExpectedPolicy::from_server_args(
        &[
            "app-server",
            "-c",
            "model='configured-model'",
            "-c",
            "sandbox_mode='workspace-write'",
            "-c",
            "approval_policy='on-request'",
            "-c",
            "model_reasoning_effort='high'",
        ]
        .map(str::to_owned),
    )
    .unwrap()
}

fn metadata(reviewer: &str) -> Value {
    json!({"thread":{"id":"saved", "canAcceptDirectInput":true,
        "status":{"type":"idle"}, "turns":[]},
        "model":"configured-model", "reasoningEffort":"high",
        "sandbox":{"type":"workspaceWrite"}, "approvalPolicy":"on-request",
        "approvalsReviewer":reviewer})
}

async fn fixture(
    case: Case,
) -> (
    Arc<CodexSharedClient>,
    Arc<Mutex<Vec<Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let server = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(server_io).await.unwrap();
        let mut resumes = 0;
        let mut evicted = false;
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let request: Value = serde_json::from_str(&text).unwrap();
            recorded.lock().unwrap().push(request.clone());
            let mut notifications = Vec::new();
            let result = match request["method"].as_str().unwrap() {
                "thread/resume" => {
                    resumes += 1;
                    if evicted && case == Case::RetainedWriter {
                        socket
                            .send(Message::Text(
                                json!({"id":request["id"],
                            "error":{"code":-32600,"message":"live writer retained"}})
                                .to_string()
                                .into(),
                            ))
                            .await
                            .unwrap();
                        continue;
                    }
                    let reviewer = if matches!(case, Case::User | Case::UserConflict) || resumes > 1
                    {
                        "user"
                    } else {
                        "auto_review"
                    };
                    let mut response = metadata(reviewer);
                    if resumes == 1 {
                        if case == Case::Busy {
                            response["thread"]["status"]["type"] = json!("active");
                        }
                        if case == Case::ForeignIdentity {
                            response["thread"]["id"] = json!("foreign");
                        }
                        if case == Case::CustomReviewer {
                            response["approvalsReviewer"] = json!("guardian_subagent");
                        }
                        match case {
                            Case::UserConflict => notifications.push(json!({
                                "method":"thread/settings/updated","params":{
                                    "threadId":"saved","threadSettings":{"approvalsReviewer":"auto_review"}}})),
                            Case::MissingReviewer => { response.as_object_mut().unwrap().remove("approvalsReviewer"); }
                            Case::WrongModel => response["model"] = json!("other-model"),
                            Case::WrongEffort => response["reasoningEffort"] = json!("low"),
                            Case::WrongApproval => response["approvalPolicy"] = json!("never"),
                            Case::WrongSandbox => response["sandbox"]["type"] = json!("dangerFullAccess"),
                            Case::NoDirectInput => response["thread"]["canAcceptDirectInput"] = json!(false),
                            Case::InProgressTurn => response["thread"]["turns"] = json!([{"status":"inProgress"}]),
                            Case::ActiveNotification => notifications.push(json!({
                                "method":"thread/status/changed","params":{
                                    "threadId":"saved","status":{"type":"active"}}})),
                            Case::TurnNotification => notifications.push(json!({
                                "method":"turn/started","params":{
                                    "threadId":"saved","turn":{"id":"unexpected"}}})),
                            Case::PrematureClosed => notifications.push(json!({
                                "method":"thread/closed","params":{"threadId":"saved"}})),
                            Case::LaggedNotifications => {
                                // The checkpoint awaits this RPC's response while
                                // the real reader fills its bounded event channel.
                                for _ in 0..64 {
                                    notifications.push(json!({"method":"thread/status/changed",
                                        "params":{"threadId":"saved","status":{"type":"idle"}}}));
                                }
                            }
                            Case::PreFenceApplied => notifications.push(json!({
                                "method":"thread/settings/updated","params":{
                                    "threadId":"saved","threadSettings":{"approvalsReviewer":"user"}}})),
                            _ => {}
                        }
                    }
                    if (resumes == 2 && case == Case::ReadbackMismatch)
                        || (evicted && case == Case::PersistenceLost)
                    {
                        response["approvalsReviewer"] = json!("auto_review");
                    }
                    response
                }
                "thread/settings/update" => {
                    assert_eq!(
                        request["params"],
                        json!({"threadId":"saved","approvalsReviewer":"user"})
                    );
                    if case == Case::Disconnect {
                        socket.close(None).await.unwrap();
                        break;
                    }
                    if case == Case::AsyncError {
                        notifications.push(json!({"method":"error","params":{"threadId":"saved",
                            "error":{"message":"private-payload-must-not-be-echoed"}}}));
                    } else if !matches!(case, Case::MissingApplied | Case::PreFenceApplied) {
                        notifications.push(json!({"method":"thread/settings/updated","params":{
                            "threadId":if case == Case::ForeignApplied {"foreign"} else {"saved"},
                            "threadSettings":{"approvalsReviewer":if case == Case::ConflictingApplied {"auto_review"} else {"user"},
                                "model":"configured-model","effort":"high"}}}));
                    }
                    json!({})
                }
                "thread/unsubscribe" => {
                    evicted = true;
                    if case != Case::MissingClosed {
                        notifications.push(json!({"method":"thread/closed","params":{
                            "threadId":if case == Case::ForeignClosed {"foreign"} else {"saved"}}}));
                    }
                    json!({"status":"unsubscribed"})
                }
                "thread/loaded/list" => {
                    json!({"data":if case == Case::StillLoaded {vec!["saved"]} else {vec![]}})
                }
                other => panic!("unexpected startup operation: {other}"),
            };
            // Both applied and closed can precede their submitted RPC receipts.
            for event in notifications {
                socket
                    .send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        }
    });
    let (socket, _) = tokio_tungstenite::client_async("ws://localhost", client_io)
        .await
        .unwrap();
    (
        CodexSharedClient::from_connected("owner".into(), 7, socket, None),
        requests,
        server,
    )
}

#[tokio::test]
async fn private_checkpoint_requires_applied_then_real_eviction_and_keeps_owner_unbound() {
    let (client, requests, server) = fixture(Case::Auto).await;
    prepare_saved_resume(&client, &policy(), "saved", &mut || Ok(()))
        .await
        .unwrap();
    assert!(client.observation.borrow().thread_id.is_none());
    let frames = requests.lock().unwrap().clone();
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "thread/resume",
            "thread/settings/update",
            "thread/resume",
            "thread/unsubscribe",
            "thread/loaded/list"
        ]
    );
    assert_eq!(
        frames[0]["params"],
        json!({"threadId":"saved", "excludeTurns":true,
        "model":"configured-model", "sandbox":"workspace-write", "approvalPolicy":"on-request",
        "config":{"model_reasoning_effort":"high"}})
    );
    assert_eq!(
        frames[2]["params"],
        json!({"threadId":"saved","excludeTurns":true})
    );
    client.close().await;
    server.await.unwrap();
}

#[tokio::test]
async fn saved_user_skips_setter_without_waiting_for_a_noop_notification() {
    let (client, requests, server) = fixture(Case::User).await;
    prepare_saved_resume(&client, &policy(), "saved", &mut || Ok(()))
        .await
        .unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .map(|frame| frame["method"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["thread/resume", "thread/unsubscribe", "thread/loaded/list"]
    );
    client.close().await;
    server.await.unwrap();
}

#[tokio::test]
async fn unsafe_source_rejects_before_any_settings_write_or_eviction() {
    for case in [
        Case::Busy,
        Case::ForeignIdentity,
        Case::CustomReviewer,
        Case::UserConflict,
        Case::MissingReviewer,
        Case::WrongModel,
        Case::WrongEffort,
        Case::WrongApproval,
        Case::WrongSandbox,
        Case::NoDirectInput,
        Case::InProgressTurn,
        Case::ActiveNotification,
        Case::TurnNotification,
        Case::PrematureClosed,
        Case::LaggedNotifications,
    ] {
        let (client, requests, server) = fixture(case).await;
        let error = prepare_saved_resume(&client, &policy(), "saved", &mut || Ok(()))
            .await
            .unwrap_err();
        assert!(!error.message.contains("timed out"), "{case:?}: {error}");
        assert_eq!(requests.lock().unwrap().len(), 1, "{case:?}");
        assert!(client.observation.borrow().thread_id.is_none());
        client.close().await;
        server.await.unwrap();
    }
}

#[tokio::test]
async fn submitted_only_wrong_or_missing_events_and_conflicts_do_not_complete_preparation() {
    for case in [
        Case::MissingApplied,
        Case::ForeignApplied,
        Case::PreFenceApplied,
        Case::ConflictingApplied,
        Case::AsyncError,
        Case::Disconnect,
        Case::ReadbackMismatch,
        Case::MissingClosed,
        Case::ForeignClosed,
        Case::StillLoaded,
    ] {
        let (client, requests, server) = fixture(case).await;
        let missing_event = matches!(
            case,
            Case::MissingApplied
                | Case::ForeignApplied
                | Case::PreFenceApplied
                | Case::MissingClosed
                | Case::ForeignClosed
        );
        let error = prepare_with_timeout(
            &client,
            &policy(),
            "saved",
            &mut || Ok(()),
            if missing_event {
                Duration::from_secs(2)
            } else {
                Duration::from_secs(5)
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.message.contains("timed out"),
            missing_event,
            "{case:?}: {error}"
        );
        assert!(!error.message.contains("private-payload"));
        assert!(client.observation.borrow().thread_id.is_none());
        let mut expected = vec!["thread/resume", "thread/settings/update"];
        if matches!(
            case,
            Case::ReadbackMismatch | Case::MissingClosed | Case::ForeignClosed | Case::StillLoaded
        ) {
            expected.push("thread/resume");
        }
        if matches!(
            case,
            Case::MissingClosed | Case::ForeignClosed | Case::StillLoaded
        ) {
            expected.push("thread/unsubscribe");
        }
        if case == Case::StillLoaded {
            expected.push("thread/loaded/list");
        }
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .map(|frame| frame["method"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            expected,
            "{case:?}"
        );
        client.close().await;
        server.await.unwrap();
    }
}

#[tokio::test]
async fn final_cold_policy_check_rejects_lost_persistence_and_retained_writer() {
    for case in [Case::PersistenceLost, Case::RetainedWriter] {
        let (client, _, server) = fixture(case).await;
        prepare_saved_resume(&client, &policy(), "saved", &mut || Ok(()))
            .await
            .unwrap();
        // Emulate the next ordinary cold loader's response, not a native TUI.
        let cold = client
            .resume_metadata(policy().initial_resume_params("saved"))
            .await;
        assert!(cold
            .and_then(|response| policy().validate(&response))
            .is_err());
        assert!(client.observation.borrow().thread_id.is_none());
        client.close().await;
        server.await.unwrap();
    }
}

#[tokio::test]
async fn already_bound_or_dead_startup_cannot_issue_private_prepare_operations() {
    let (client, requests, server) = fixture(Case::Auto).await;
    client.bind(&metadata("user")).unwrap();
    assert!(
        prepare_saved_resume(&client, &policy(), "saved", &mut || Ok(()))
            .await
            .is_err()
    );
    assert!(requests.lock().unwrap().is_empty());
    client.close().await;
    server.await.unwrap();
    let (client, requests, server) = fixture(Case::Auto).await;
    assert!(
        prepare_saved_resume(&client, &policy(), "saved", &mut || Err(
            CodexSharedError::unsupported("child ended")
        ))
        .await
        .is_err()
    );
    assert!(requests.lock().unwrap().is_empty());
    client.close().await;
    server.await.unwrap();
}

#[tokio::test]
async fn ineligible_version_or_expired_deadline_rejects_before_preload() {
    for version in [
        "0.154.0",
        "0.154.0-alpha.6",
        "0.159.99",
        "0.160.0-alpha.1",
        "unknown",
    ] {
        let (client, requests, server) = fixture(Case::Auto).await;
        let error = super::prepare_saved_resume(
            &client,
            &policy(),
            "saved",
            &mut || Ok(()),
            tokio::time::Instant::now() + STARTUP_TIMEOUT,
            version,
        )
        .await
        .unwrap_err();
        assert!(!error.provider_boundary_crossed);
        assert!(requests.lock().unwrap().is_empty(), "{version}");
        client.close().await;
        server.await.unwrap();
    }
    let (client, requests, server) = fixture(Case::Auto).await;
    let error = super::prepare_saved_resume(
        &client,
        &policy(),
        "saved",
        &mut || Ok(()),
        tokio::time::Instant::now(),
        "0.160.0",
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("budget exhausted"));
    assert!(!error.provider_boundary_crossed);
    assert!(requests.lock().unwrap().is_empty());
    client.close().await;
    server.await.unwrap();
}

#[tokio::test]
async fn initial_fence_is_rechecked_after_acquiring_the_real_writer_lock() {
    for bind_owner in [false, true] {
        let (client, requests, server) = fixture(Case::Auto).await;
        let writer = client.writer.lock().await;
        let requester = client.clone();
        let waiting = tokio::spawn(async move {
            requester
                .request_with_fences(
                    "thread/settings/update",
                    json!({"threadId":"saved","approvalsReviewer":"user"}),
                    Some(Duration::from_secs(5)),
                    None,
                    Some(0),
                )
                .await
        });
        // Pending registration precedes lock acquisition; no sleeps or guessed
        // scheduling order stand in for reaching that real write boundary.
        tokio::time::timeout(Duration::from_secs(5), async {
            while client.pending.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if bind_owner {
            client.bind(&metadata("user")).unwrap();
        } else {
            client
                .initial_activity_sequence
                .fetch_add(1, Ordering::AcqRel);
        }
        drop(writer);
        let error = waiting.await.unwrap().unwrap_err();
        assert_eq!(error.code, "unsupported");
        assert!(error.message.contains("changed before write"));
        assert!(!error.provider_boundary_crossed);
        assert!(client.pending.lock().unwrap().is_empty());
        assert!(requests.lock().unwrap().is_empty());
        client.close().await;
        server.await.unwrap();
    }
}
