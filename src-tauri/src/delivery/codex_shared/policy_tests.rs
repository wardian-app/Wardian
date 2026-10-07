use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use wardian_core::models::{AgentConfig, CodexProviderConfig, ProviderConfig};

fn configured_args(sandbox: &str, approval: &str, full_auto: bool) -> Vec<String> {
    let config = AgentConfig {
        provider: "codex".into(),
        model: Some("configured-model".into()),
        provider_config: ProviderConfig::Codex(CodexProviderConfig {
            sandbox_mode: Some(sandbox.into()),
            approval_policy: Some(approval.into()),
            full_auto: Some(full_auto),
            reasoning_effort: Some("high".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    crate::providers::CodexProvider::new()
        .shared_server_args(&config)
        .unwrap()
}

#[test]
fn generated_policy_produces_stock_direct_launch_choices() {
    for (sandbox, approval, full_auto, expected) in [
        (
            "read-only",
            "never",
            false,
            vec!["--sandbox", "read-only", "--ask-for-approval", "never"],
        ),
        (
            "workspace-write",
            "on-request",
            false,
            vec![
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
            ],
        ),
        (
            "workspace-write",
            "on-request",
            true,
            vec!["--dangerously-bypass-approvals-and-sandbox"],
        ),
    ] {
        let args = configured_args(sandbox, approval, full_auto);
        let policy = ExpectedPolicy::from_server_args(&args).unwrap();
        assert_eq!(policy.tui_permission_args().unwrap(), expected);
    }
}

#[test]
fn unsupported_reviewer_and_incomplete_or_invalid_choices_fail_closed() {
    let guardian = ExpectedPolicy::from_server_args(&configured_args(
        "workspace-write",
        "approve-for-me",
        false,
    ))
    .unwrap();
    let error = guardian.tui_permission_args().unwrap_err();
    assert_eq!(error.code, "unsupported");
    assert!(!error.provider_boundary_crossed);
    assert!(error.message.contains("reviewer"));
    for overrides in [
        vec![],
        vec!["sandbox_mode='workspace-write'"],
        vec!["sandbox_mode='unsafe-value'", "approval_policy='never'"],
        vec![
            "sandbox_mode='workspace-write'",
            "approval_policy='invalid-choice'",
        ],
    ] {
        let args = overrides
            .into_iter()
            .flat_map(|value| ["-c".to_owned(), value.to_owned()])
            .collect::<Vec<_>>();
        assert!(ExpectedPolicy::from_server_args(&args)
            .unwrap()
            .tui_permission_args()
            .is_err());
    }
}

#[tokio::test]
async fn both_cold_load_paths_serialize_captured_policy_on_the_actual_transport() {
    for (sandbox, approval, full_auto, expected_sandbox, expected_approval) in [
        ("read-only", "never", false, "read-only", "never"),
        (
            "workspace-write",
            "on-request",
            false,
            "workspace-write",
            "on-request",
        ),
        (
            "workspace-write",
            "on-request",
            true,
            "danger-full-access",
            "never",
        ),
    ] {
        let args = configured_args(sandbox, approval, full_auto);
        let policy = ExpectedPolicy::from_server_args(&args).unwrap();
        assert!(policy.tui_permission_args().is_ok());
        for resume_id in [None, Some("saved-thread")] {
            let mut expected = json!({
                "model":"configured-model", "config":{"model_reasoning_effort":"high"},
                "sandbox":expected_sandbox, "approvalPolicy":expected_approval, "approvalsReviewer":"user",
            });
            let method = if let Some(id) = resume_id {
                expected["threadId"] = json!(id);
                expected["excludeTurns"] = json!(true);
                "thread/resume"
            } else {
                expected["cwd"] = json!("workspace");
                "thread/start"
            };
            let response_sandbox = match expected_sandbox {
                "read-only" => "readOnly",
                "workspace-write" => "workspaceWrite",
                "danger-full-access" => "dangerFullAccess",
                _ => unreachable!(),
            };
            let (client_io, server_io) = tokio::io::duplex(4096);
            let server = tokio::spawn(async move {
                let mut socket = tokio_tungstenite::accept_async(server_io).await.unwrap();
                let request: Value =
                    serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                assert_eq!(request["method"], method);
                assert_eq!(request["params"], expected);
                socket
                    .send(Message::Text(
                        json!({"id":request["id"], "result":{
                            "thread":{"id":"saved-thread"}, "model":"configured-model",
                            "reasoningEffort":"high", "sandbox":{"type":response_sandbox},
                            "approvalPolicy":expected_approval, "approvalsReviewer":"user",
                        }})
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            });
            let (socket, _) = tokio_tungstenite::client_async("ws://localhost", client_io)
                .await
                .unwrap();
            let client = CodexSharedClient::from_connected("test-agent".into(), 7, socket, None);
            let response = policy
                .load_background_thread(&client, resume_id, Path::new("workspace"))
                .await
                .unwrap();
            assert_eq!(response["thread"]["id"], "saved-thread");
            assert!(policy.validate(&response).is_ok());
            client.close().await;
            server.await.unwrap();
        }
    }
}

#[test]
fn generated_policy_rejects_inherited_reviewers_on_attachment() {
    for (sandbox, approval, full_auto, response_sandbox, response_approval) in [
        ("read-only", "never", false, "readOnly", "never"),
        (
            "workspace-write",
            "on-request",
            false,
            "workspaceWrite",
            "on-request",
        ),
        (
            "workspace-write",
            "on-request",
            true,
            "dangerFullAccess",
            "never",
        ),
    ] {
        let args = configured_args(sandbox, approval, full_auto);
        let policy = ExpectedPolicy::from_server_args(&args).unwrap();
        assert!(policy.tui_permission_args().is_ok());
        let response = json!({
            "model":"configured-model", "reasoningEffort":"high",
            "sandbox":{"type":response_sandbox}, "approvalPolicy":response_approval,
            "approvalsReviewer":"user",
        });
        assert!(policy.validate(&response).is_ok());
        for reviewer in [
            json!("auto_review"),
            json!("guardian_subagent"),
            json!("human"),
            Value::Null,
            json!({"secret":"private-token"}),
        ] {
            let mut inherited = response.clone();
            inherited["approvalsReviewer"] = reviewer;
            let error = policy.validate(&inherited).unwrap_err();
            assert_eq!(error.code, "unsupported");
            assert!(error.message.contains("approvalsReviewer"));
            assert!(error.message.contains("expected=user"));
            assert!(!error.message.contains("private-token"));
        }
        let mut missing = response;
        missing.as_object_mut().unwrap().remove("approvalsReviewer");
        assert!(policy.validate(&missing).is_err());
    }
}

#[test]
fn policy_rejection_diagnostics_are_bounded_and_whitelisted() {
    let policy =
        ExpectedPolicy::from_server_args(&["-c".into(), "sandbox_mode='workspace-write'".into()])
            .unwrap();
    let mismatch = policy
        .validate(&json!({"sandbox":{"type":"readOnly"}}))
        .unwrap_err();
    assert!(mismatch
        .message
        .contains("expected=workspaceWrite, actual=readOnly"));
    for response in [
        json!({}),
        json!({"sandbox":{"type":{"secret":"private-token"}}}),
        json!({"sandbox":{"type":"/private/secret/private-token"},"other":"private-token"}),
    ] {
        let error = policy.validate(&response).unwrap_err();
        assert!(error.message.len() < 200);
        assert!(!error.message.contains("private-token"));
        assert!(!error.message.contains("/private"));
        assert!(!error.provider_boundary_crossed);
    }
}
