use super::*;
use futures_util::{SinkExt, StreamExt};
use std::fs;
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
        let mut policy = ExpectedPolicy::from_server_args(&args).unwrap();
        policy
            .resolve_windows_sandbox(&json!({"config":{},"origins":{}}), true)
            .unwrap();
        assert!(policy.tui_permission_args().is_ok());
        for resume_id in [None, Some("saved-thread")] {
            let mut expected = json!({
                "model":"configured-model", "config":{"model_reasoning_effort":"high"},
                "sandbox":expected_sandbox, "approvalPolicy":expected_approval, "approvalsReviewer":"user",
            });
            if sandbox == "workspace-write" && !full_auto {
                expected["config"]["windows.sandbox"] = json!("unelevated");
                expected["config"]["features.prefer_mxc"] = json!(false);
            }
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
fn windows_backend_defaults_only_for_unselected_workspace_write_on_windows() {
    for (sandbox, full_auto, is_windows, expected) in [
        (
            "workspace-write",
            false,
            true,
            Some(WindowsSandboxMode::Unelevated),
        ),
        ("read-only", false, true, None),
        ("danger-full-access", true, true, None),
        ("workspace-write", false, false, None),
    ] {
        let args = configured_args(sandbox, "on-request", full_auto);
        let mut policy = ExpectedPolicy::from_server_args(&args).unwrap();
        policy
            .resolve_windows_sandbox(&json!({"config":{},"origins":{}}), is_windows)
            .unwrap();
        assert_eq!(policy.windows_sandbox, expected);
    }
}

#[test]
fn windows_backend_preserves_modern_and_legacy_selections() {
    for (backend, expected) in [
        ("elevated", WindowsSandboxMode::Elevated),
        ("mxc", WindowsSandboxMode::Mxc),
        ("unelevated", WindowsSandboxMode::Unelevated),
    ] {
        let config = json!({"windows":{"sandbox":backend}});
        let origins = json!({"windows.sandbox":{"name":{"type":"project","dotCodexFolder":"/workspace/.codex"},"version":"1"}});
        let mut policy = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        policy
            .resolve_windows_sandbox(&json!({"config":config,"origins":origins}), true)
            .unwrap();
        assert_eq!(policy.windows_sandbox, Some(expected));
    }

    for (feature, enabled, expected) in [
        (
            "experimental_windows_sandbox",
            true,
            Some(WindowsSandboxMode::Unelevated),
        ),
        (
            "enable_experimental_windows_sandbox",
            true,
            Some(WindowsSandboxMode::Unelevated),
        ),
        (
            "elevated_windows_sandbox",
            true,
            Some(WindowsSandboxMode::Elevated),
        ),
    ] {
        let mut feature_config = serde_json::Map::new();
        feature_config.insert(feature.into(), json!(enabled));
        let config = json!({"features":feature_config});
        let mut origins = serde_json::Map::new();
        origins.insert(
            format!("features.{feature}"),
            json!({"name":{"type":"project","dotCodexFolder":"/workspace/.codex"},"version":"1"}),
        );
        let mut policy = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        policy
            .resolve_windows_sandbox(&json!({"config":config,"origins":origins}), true)
            .unwrap();
        assert_eq!(policy.windows_sandbox, expected, "feature={feature}");
    }

    let mut policy =
        ExpectedPolicy::from_server_args(&configured_args("workspace-write", "on-request", false))
            .unwrap();
    policy
        .resolve_windows_sandbox(
            &json!({
                "config":{"windows":{"sandbox":"elevated"},"features":{"experimental_windows_sandbox":true}},
                "origins":{
                    "windows.sandbox":{"name":{"type":"project","dotCodexFolder":"/workspace/.codex"},"version":"1"},
                    "features.experimental_windows_sandbox":{"name":{"type":"user","file":"/codex/config.toml"},"version":"1"}
                }
            }),
            true,
        )
        .unwrap();
    assert_eq!(policy.windows_sandbox, Some(WindowsSandboxMode::Elevated));
}

#[test]
fn windows_backend_preserves_managed_exact_choices_without_origins() {
    for (config, expected) in [
        (
            json!({"windows":{"sandbox":"elevated"}}),
            Some(WindowsSandboxMode::Elevated),
        ),
        (
            json!({"features":{"experimental_windows_sandbox":true}}),
            Some(WindowsSandboxMode::Unelevated),
        ),
        (
            json!({"features":{"elevated_windows_sandbox":true}}),
            Some(WindowsSandboxMode::Elevated),
        ),
        (
            json!({"features":{"prefer_mxc":false}}),
            Some(WindowsSandboxMode::Unelevated),
        ),
    ] {
        let mut policy = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        policy
            .resolve_windows_sandbox(&json!({"config":config,"origins":{}}), true)
            .unwrap();
        assert_eq!(policy.windows_sandbox, expected, "config={config}");
    }
}

#[test]
fn conditional_prefer_mxc_must_be_resolved_or_rejected_for_workspace_write() {
    for config in [
        json!({"features":{"prefer_mxc":true}}),
        json!({"windows":{"sandbox":"elevated"},"features":{"prefer_mxc":true}}),
        json!({"windows":{"sandbox":"unelevated"},"features":{"prefer_mxc":true}}),
        json!({"features":{"experimental_windows_sandbox":true,"prefer_mxc":true}}),
        json!({"features":{"elevated_windows_sandbox":true,"prefer_mxc":true}}),
    ] {
        let mut policy = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        assert!(policy
            .resolve_windows_sandbox(&json!({"config":config,"origins":{}}), true)
            .is_err());
    }

    let mut explicit_mxc =
        ExpectedPolicy::from_server_args(&configured_args("workspace-write", "on-request", false))
            .unwrap();
    explicit_mxc
        .resolve_windows_sandbox(
            &json!({
                "config":{"windows":{"sandbox":"mxc"},"features":{"prefer_mxc":true}},
                "origins":{}
            }),
            true,
        )
        .unwrap();
    assert_eq!(explicit_mxc.windows_sandbox, Some(WindowsSandboxMode::Mxc));

    let mut read_only =
        ExpectedPolicy::from_server_args(&configured_args("read-only", "never", false)).unwrap();
    read_only
        .resolve_windows_sandbox(
            &json!({
                "config":{"windows":{"sandbox":"elevated"},"features":{"prefer_mxc":true}},
                "origins":{}
            }),
            true,
        )
        .unwrap();
    assert_eq!(read_only.windows_sandbox, None);
    assert!(read_only.background_resume_params("saved-thread")["config"]
        .get("windows.sandbox")
        .is_none());
    let mut read_only_overlay = Vec::new();
    read_only.append_windows_sandbox_arg(&mut read_only_overlay);
    assert!(read_only_overlay.is_empty());

    for config in [
        json!({"features":{"prefer_mxc":true}}),
        json!({
            "windows":{"sandbox":"elevated"},
            "features":{"prefer_mxc":true}
        }),
    ] {
        let mut full_access = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            true,
        ))
        .unwrap();
        full_access
            .resolve_windows_sandbox(&json!({"config":config,"origins":{}}), true)
            .unwrap();
        assert_eq!(full_access.windows_sandbox, None);
        assert!(
            full_access.background_resume_params("saved-thread")["config"]
                .get("windows.sandbox")
                .is_none()
        );
        let mut full_access_overlay = Vec::new();
        full_access.append_windows_sandbox_arg(&mut full_access_overlay);
        assert!(full_access_overlay.is_empty());
    }
}

#[test]
fn workspace_write_rejects_legacy_disabled_windows_sandbox_before_load() {
    for feature in ["experimental_windows_sandbox", "elevated_windows_sandbox"] {
        let mut features = serde_json::Map::new();
        features.insert(feature.into(), json!(false));
        features.insert("prefer_mxc".into(), json!(false));
        let config = json!({"features":features});
        let mut workspace_write = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        assert!(workspace_write
            .resolve_windows_sandbox(&json!({"config":config.clone(),"origins":{}}), true)
            .is_err());

        let mut read_only =
            ExpectedPolicy::from_server_args(&configured_args("read-only", "never", false))
                .unwrap();
        read_only
            .resolve_windows_sandbox(&json!({"config":config,"origins":{}}), true)
            .unwrap();
        assert_eq!(read_only.windows_sandbox, None);
    }
}

#[test]
fn windows_sandbox_service_is_validated_without_selecting_backend() {
    let managed_origin = json!({
        "name":{
            "type":"enterpriseManaged",
            "id":"managed",
            "name":"policy"
        },
        "version":"1"
    });
    for service_enabled in [false, true] {
        for origin in [None, Some(managed_origin.clone())] {
            let has_origin = origin.is_some();
            let mut origins = serde_json::Map::new();
            if let Some(metadata) = origin {
                origins.insert("features.windows_sandbox_service".into(), metadata);
            }
            let config_read = json!({
                "config":{"features":{"windows_sandbox_service":service_enabled}},
                "origins":origins,
                "requirements":null
            });
            let mut policy = ExpectedPolicy::from_server_args(&configured_args(
                "workspace-write",
                "on-request",
                false,
            ))
            .unwrap();
            policy.resolve_windows_sandbox(&config_read, true).unwrap();
            assert_eq!(
                policy.windows_sandbox,
                Some(WindowsSandboxMode::Unelevated),
                "service_enabled={service_enabled}, has_origin={has_origin}"
            );
            assert_eq!(
                config_read["config"]["features"]["windows_sandbox_service"],
                json!(service_enabled)
            );
        }
    }

    for service_enabled in [false, true] {
        for (mut config, expected) in [
            (
                json!({"windows":{"sandbox":"elevated"}}),
                WindowsSandboxMode::Elevated,
            ),
            (
                json!({"features":{"experimental_windows_sandbox":true}}),
                WindowsSandboxMode::Unelevated,
            ),
            (
                json!({"features":{"elevated_windows_sandbox":true}}),
                WindowsSandboxMode::Elevated,
            ),
        ] {
            let config_object = config.as_object_mut().unwrap();
            let features = config_object
                .entry("features".to_owned())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            features.insert("windows_sandbox_service".into(), json!(service_enabled));
            let mut policy = ExpectedPolicy::from_server_args(&configured_args(
                "workspace-write",
                "on-request",
                false,
            ))
            .unwrap();
            policy
                .resolve_windows_sandbox(&json!({"config":config,"origins":{}}), true)
                .unwrap();
            assert_eq!(policy.windows_sandbox, Some(expected));
        }
    }

    for (config_read, expected_error) in [
        (
            json!({
                "config":{"features":{"windows_sandbox_service":"false"}},
                "origins":{}
            }),
            "Windows sandbox feature value is malformed",
        ),
        (
            json!({
                "config":{"features":{"windows_sandbox_service":false}},
                "origins":{"features.windows_sandbox_service":"malformed"}
            }),
            "malformed source origin metadata",
        ),
        (
            json!({
                "config":{"features":{}},
                "origins":{
                    "features.windows_sandbox_service":managed_origin.clone()
                }
            }),
            "feature origin has no effective value",
        ),
    ] {
        let mut policy = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        let error = policy
            .resolve_windows_sandbox(&config_read, true)
            .unwrap_err();
        assert!(
            error.message.contains(expected_error),
            "unexpected error for config/read: {config_read}: {}",
            error.message
        );
    }

    let managed_disabled_backend = json!({
        "config":{"features":{"experimental_windows_sandbox":false}},
        "origins":{
            "features.experimental_windows_sandbox":managed_origin
        }
    });
    let mut policy =
        ExpectedPolicy::from_server_args(&configured_args("workspace-write", "on-request", false))
            .unwrap();
    let error = policy
        .resolve_windows_sandbox(&managed_disabled_backend, true)
        .unwrap_err();
    assert!(error
        .message
        .contains("legacy Windows sandbox selection disables"));
}

#[test]
fn every_selected_windows_backend_pins_prefer_mxc_false_in_both_outputs() {
    for (config, expected) in [
        (json!({}), WindowsSandboxMode::Unelevated),
        (
            json!({"features":{"prefer_mxc":false}}),
            WindowsSandboxMode::Unelevated,
        ),
        (
            json!({"windows":{"sandbox":"elevated"}}),
            WindowsSandboxMode::Elevated,
        ),
        (
            json!({"windows":{"sandbox":"mxc"},"features":{"prefer_mxc":true}}),
            WindowsSandboxMode::Mxc,
        ),
    ] {
        let mut policy = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        policy
            .resolve_windows_sandbox(&json!({"config":config,"origins":{}}), true)
            .unwrap();
        assert_eq!(policy.windows_sandbox, Some(expected));

        let params = policy.background_resume_params("saved-thread");
        assert_eq!(
            params["config"]["windows.sandbox"],
            json!(expected.as_str())
        );
        assert_eq!(params["config"]["features.prefer_mxc"], json!(false));

        let mut overlay_args = Vec::new();
        policy.append_windows_sandbox_arg(&mut overlay_args);
        assert_eq!(
            overlay_args,
            vec![
                "-c".to_owned(),
                format!(r#"windows.sandbox="{}""#, expected.as_str()),
                "-c".to_owned(),
                "features.prefer_mxc=false".to_owned(),
            ]
        );
    }
}

#[test]
fn windows_backend_fails_closed_when_effective_source_is_ambiguous() {
    for (case, config_read) in [
        ("missing effective config", json!({"config":{}})),
        ("null origins", json!({"config":{},"origins":null})),
        ("non-object origins", json!({"config":{},"origins":[]})),
        (
            "unsupported backend value",
            json!({
                "config":{"windows":{"sandbox":"future-backend"}},
                "origins":{"windows.sandbox":{"name":{"type":"project","dotCodexFolder":"/workspace/.codex"},"version":"1"}}
            }),
        ),
        (
            "non-object metadata",
            json!({
                "config":{"windows":{"sandbox":"elevated"}},
                "origins":{"windows.sandbox":"managed"}
            }),
        ),
        (
            "origin without effective value",
            json!({
                "config":{},
                "origins":{"windows.sandbox":{"name":{"type":"project","dotCodexFolder":"/workspace/.codex"},"version":"1"}}
            }),
        ),
    ] {
        let mut policy = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        let result = policy.resolve_windows_sandbox(&config_read, true);
        assert!(result.is_err(), "accepted config/read case: {case}");
    }

    let valid_project_name = json!({
        "type":"project",
        "dotCodexFolder":"/workspace/.codex"
    });
    for (case, metadata) in [
        ("empty metadata", json!({})),
        ("missing source name", json!({"version":"1"})),
        (
            "missing metadata version",
            json!({"name":valid_project_name.clone()}),
        ),
        (
            "non-object source name",
            json!({"name":"project","version":"1"}),
        ),
        (
            "non-string metadata version",
            json!({"name":valid_project_name.clone(),"version":1}),
        ),
        (
            "missing project source field",
            json!({"name":{"type":"project"},"version":"1"}),
        ),
        (
            "non-string project source field",
            json!({"name":{"type":"project","dotCodexFolder":1},"version":"1"}),
        ),
        (
            "unknown source type",
            json!({"name":{"type":"futureSource"},"version":"1"}),
        ),
    ] {
        let mut policy = ExpectedPolicy::from_server_args(&configured_args(
            "workspace-write",
            "on-request",
            false,
        ))
        .unwrap();
        let config_read = json!({
            "config":{"features":{"experimental_windows_sandbox":true}},
            "origins":{"features.experimental_windows_sandbox":metadata}
        });
        let error = policy
            .resolve_windows_sandbox(&config_read, true)
            .unwrap_err();
        assert!(
            error.message.contains("malformed source origin metadata"),
            "config/read case {case} returned: {}",
            error.message
        );
    }

    let mut non_windows =
        ExpectedPolicy::from_server_args(&configured_args("workspace-write", "on-request", false))
            .unwrap();
    non_windows
        .resolve_windows_sandbox(&json!({"config":"malformed"}), false)
        .unwrap();
    assert_eq!(non_windows.windows_sandbox, None);
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
fn generated_args_overlay_pins_inherited_reviewer_before_attachment() {
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

        let home = tempfile::tempdir().unwrap();
        let config_path = home.path().join("config.toml");
        fs::write(&config_path, "approvals_reviewer = 'auto_review'\n").unwrap();
        let mut guard =
            super::super::launch_config::prepare_tui_launch_config(home.path(), &args).unwrap();
        let overlay: toml_edit::DocumentMut =
            fs::read_to_string(&config_path).unwrap().parse().unwrap();
        let response = json!({
            "model":"configured-model", "reasoningEffort":"high",
            "sandbox":{"type":response_sandbox}, "approvalPolicy":response_approval,
            "approvalsReviewer":overlay["approvals_reviewer"].as_str().unwrap(),
        });
        policy
            .validate(&response)
            .unwrap_or_else(|error| panic!("{}", error.message));

        guard.restore().unwrap();
        let restored: toml_edit::DocumentMut =
            fs::read_to_string(&config_path).unwrap().parse().unwrap();
        assert_eq!(restored["approvals_reviewer"].as_str(), Some("auto_review"));

        let mut guard =
            super::super::launch_config::prepare_tui_launch_config(home.path(), &args).unwrap();
        let mut externally_edited: toml_edit::DocumentMut =
            fs::read_to_string(&config_path).unwrap().parse().unwrap();
        externally_edited["approvals_reviewer"] = toml_edit::value("human");
        fs::write(&config_path, externally_edited.to_string()).unwrap();
        guard.restore().unwrap();
        let restored: toml_edit::DocumentMut =
            fs::read_to_string(&config_path).unwrap().parse().unwrap();
        assert_eq!(restored["approvals_reviewer"].as_str(), Some("human"));
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
