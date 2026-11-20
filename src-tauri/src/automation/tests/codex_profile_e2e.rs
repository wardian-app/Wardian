use super::*;
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

async fn mcp(Json(request): Json<Value>) -> impl IntoResponse {
    let result =
        match request["method"].as_str() {
            Some("initialize") => json!({
                "protocolVersion": request["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "agent-only fixture", "version": "1"}
            }),
            Some("tools/list") => {
                let tools = ["allowed_marker", "filtered_marker"].map(|name| json!({
                "name": name, "description": "Fixture marker", "inputSchema": {"type": "object"}
            }));
                json!({"tools": tools})
            }
            Some("notifications/initialized") => return StatusCode::ACCEPTED.into_response(),
            _ => return StatusCode::BAD_REQUEST.into_response(),
        };
    Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result})).into_response()
}

async fn responses(
    State(receipts): State<mpsc::UnboundedSender<Value>>,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    let _ = receipts.send(request["tools"].clone());
    let message = json!({
        "id": "msg_fixture",
        "type": "message",
        "role": "assistant",
        "status": "completed",
        "content": [{"type": "output_text", "text": "fixture complete", "annotations": []}]
    });
    let response = json!({
        "id": "resp_fixture",
        "status": "completed",
        "output": [message],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
    });
    let events = [
        json!({"type": "response.created", "response": {"id": "resp_fixture"}}),
        json!({"type": "response.output_item.added", "item": message}),
        json!({"type": "response.output_item.done", "item": message}),
        json!({"type": "response.completed", "response": response}),
    ];
    let sse: String = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    ([(header::CONTENT_TYPE, "text/event-stream")], sse)
}

struct UpstreamHome(Option<std::ffi::OsString>);

impl Drop for UpstreamHome {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }
}

#[tokio::test]
async fn fresh_background_codex_exposes_only_the_selected_profiles_allowed_tools() {
    // Hosted unit CI does not provision the real Codex executable.
    if std::env::var("WARDIAN_E2E_REAL_CODEX_PROFILE").as_deref() != Ok("1") {
        return;
    }
    let home = TestWardianHome::new_async().await;
    let upstream = home._home.path().join("upstream");
    let workspace = home._home.path().join("workspace");
    std::fs::create_dir(&upstream).unwrap();
    std::fs::create_dir(&workspace).unwrap();
    let _restore_upstream = UpstreamHome(std::env::var_os("CODEX_HOME"));
    std::env::set_var("CODEX_HOME", &upstream);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (receipts, mut requests) = mpsc::unbounded_channel();
    let router = Router::new()
        .route(
            "/mcp",
            post(mcp).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .route("/v1/responses", post(responses))
        .route("/v1/models", get(|| async { Json(json!({"models": []})) }))
        .with_state(receipts);
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    std::fs::write(
        upstream.join("config.toml"),
        format!(
            "model_provider = 'fixture'\nmodel = 'gpt-5'\n[model_providers.fixture]\nname = 'fixture'\nbase_url = 'http://{address}/v1'\nwire_api = 'responses'\n"
        ),
    )
    .unwrap();
    let agent_id = "selected-agent";
    let agent_home = crate::utils::fs::habitat_codex_home(
        &crate::utils::fs::prepare_habitat_workspace(&workspace, "", agent_id).unwrap(),
    );
    std::fs::create_dir(&agent_home).unwrap();
    let agent_config = format!(
        "[mcp_servers.profile_fixture]\nurl = 'http://{address}/mcp'\nrequired = true\nenabled_tools = ['allowed_marker']\n"
    );
    std::fs::write(agent_home.join("config.toml"), &agent_config).unwrap();

    let agent = AgentBinding {
        session_id: agent_id.into(),
        provider: "codex".into(),
        cwd: workspace.clone(),
        resume_session: Some("profile-session-must-not-resume".into()),
        is_live: true,
        is_input_ready: true,
        config: None,
    };
    let executor = LiveStepExecutor::new(
        Arc::new(crate::automation::runner::HeadlessAgentRunner),
        workspace,
        "codex".into(),
        HashMap::new(),
        HashMap::new(),
    );
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        executor.run_background_fresh("fixture", &agent, "Respond with fixture complete".into()),
    )
    .await;
    server.abort();
    let output = output.unwrap().unwrap();
    assert_eq!(output.trim(), "fixture complete");
    let tools = requests
        .try_recv()
        .expect("Codex submitted a model request");
    let tools = tools.as_array().expect("model request includes tools");
    // Codex versions can send qualified flat functions or a tool namespace.
    let has_tool = |name: &str| {
        let qualified = format!("mcp__profile_fixture__{name}");
        tools.iter().any(|tool| {
            tool["name"] == qualified
                || (tool["name"] == "mcp__profile_fixture"
                    && tool["tools"]
                        .as_array()
                        .is_some_and(|entries| entries.iter().any(|entry| entry["name"] == name)))
        })
    };
    assert!(has_tool("allowed_marker"), "allowed MCP tool is missing");
    assert!(
        !has_tool("filtered_marker"),
        "filtered MCP tool was exposed"
    );
    assert_eq!(
        std::fs::read_to_string(agent_home.join("config.toml")).unwrap(),
        agent_config
    );
}
