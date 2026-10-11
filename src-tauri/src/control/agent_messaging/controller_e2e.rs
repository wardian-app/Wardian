//! Opt-in full HTTP/real CLI/native canonical acceptance with an isolated owner home.
use super::*;
use crate::control::test_support::TestWardianHome;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use wardian_core::control::{ControlRequest, ReplyStatus};
const WORKER: &str = "ea503f0d-e934-4c72-9185-3e780b889c29";

async fn exchange<T: AsyncRead + AsyncWrite + Unpin>(stream: T, state: Arc<AppState>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let ControlRequest::ControllerMessaging {
        request,
        token,
        resource,
    } = serde_json::from_str(&line).unwrap()
    else {
        panic!("controller ingress required")
    };
    let task = matches!(request, Request::FollowupTask { .. });
    let information = matches!(request, Request::SendMessage { .. });
    let response = handle_in_state(None, &state, request, &token, &resource).await;
    if information && response.is_ok() {
        assert!(
            state
                .interactions
                .claim_agent_task(WORKER, 0)
                .await
                .unwrap()
                .is_none(),
            "information must not enqueue provider work"
        );
    }
    if task && response.is_ok() {
        if let Some(claim) = state
            .interactions
            .claim_agent_task(WORKER, 0)
            .await
            .unwrap()
        {
            state
                .interactions
                .finish_agent_task(&claim, "provider_visible")
                .await
                .unwrap();
            // The mock worker still passes the production managed identity gate.
            super::super::handle_in_state(
                None,
                &state,
                Request::Reply {
                    request_id: claim.record.id,
                    status: ReplyStatus::Done,
                    message: "external-controller-complete".into(),
                },
                MessageOrigin::WardianAgent {
                    session_id: WORKER.into(),
                },
            )
            .await
            .unwrap();
        }
    }
    let value = match response {
        Ok(response) => serde_json::to_value(response).unwrap(),
        Err(error) => serde_json::json!({"error":{"code":error.code,"message":error.message}}),
    };
    reader
        .get_mut()
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
    reader.get_mut().flush().await.unwrap();
}

pub(super) async fn run_http_real_cli_canonical_task_reply() {
    let home = TestWardianHome::new_async().await;
    let cli = std::env::var_os("WARDIAN_BRIDGE_E2E_CLI").expect("built CLI path required");
    let driver = std::env::var_os("WARDIAN_BRIDGE_E2E_DRIVER").expect("Node driver path required");
    let state = Arc::new(AppState::new());
    let agent = crate::control::tests::test_agent(WORKER, "Fixture worker", "Test");
    *agent.current_status.lock().unwrap() = "Off".into();
    state.agents.lock().await.insert(WORKER.into(), agent);
    let resource = "https://bridge.example/mcp";
    let grant = ControllerStore::current()
        .unwrap()
        .create(
            "Fixture controller",
            resource,
            vec![WORKER.into()],
            vec![
                Scope::AgentsRead,
                Scope::MessagesSend,
                Scope::TasksSubmit,
                Scope::AgentsWake,
                Scope::ControllerReceive,
                Scope::ControllerAck,
                Scope::ControllerWait,
            ],
            None,
        )
        .unwrap();
    #[cfg(windows)]
    let listener = {
        use tokio::net::windows::named_pipe::ServerOptions;
        let pipe_name = wardian_core::control::pipe_name().unwrap();
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();
        let state = state.clone();
        tokio::spawn(async move {
            crate::control::windows_pipe::serve(&pipe_name, first, move |pipe| {
                tokio::spawn(exchange(pipe, state.clone()));
            })
            .await
            .unwrap();
        })
    };
    #[cfg(unix)]
    let listener = {
        let path = wardian_core::control::socket_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let socket = tokio::net::UnixListener::bind(path).unwrap();
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = socket.accept().await.unwrap();
                tokio::spawn(exchange(stream, state.clone()));
            }
        })
    };
    let node = std::env::var_os("WARDIAN_BRIDGE_E2E_NODE").unwrap_or_else(|| "node".into());
    let mut command = tokio::process::Command::new(node);
    command
        .arg(driver)
        .env("WARDIAN_HOME", home.path())
        .env("WARDIAN_BRIDGE_E2E_CLI", cli)
        .env("WARDIAN_BRIDGE_E2E_TOKEN", &grant.token)
        .env("WARDIAN_BRIDGE_E2E_RESOURCE", resource)
        .env("WARDIAN_BRIDGE_E2E_AGENT", WORKER)
        .env("WARDIAN_BRIDGE_TEST_AUDIT", home.path().join("audit.jsonl"))
        .env_remove("WARDIAN_SESSION_ID")
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(90), command.output()).await;
    listener.abort();
    let _ = listener.await;
    let output = output
        .expect("bounded HTTP e2e driver")
        .expect("launch Node driver");
    let stderr = String::from_utf8_lossy(&output.stderr).replace(&grant.token, "[REDACTED]");
    assert!(output.status.success(), "HTTP e2e driver failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "native_controller_http_roundtrip_passed",
        "driver must complete the actual HTTP acceptance assertions"
    );
    eprintln!("native_controller_http_roundtrip_passed");
    assert_eq!(
        state.agents.lock().await.len(),
        1,
        "controller must not become a worker"
    );
    assert!(
        !state
            .interactions
            .has_unacknowledged_agent_messages(&grant.principal.mailbox_owner())
            .await
            .unwrap(),
        "reply acknowledgement commits to the canonical controller mailbox"
    );
    let count: i64 = store::with_db(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM interactions WHERE kind='task' AND sender_session_id=?1",
            [grant.principal.mailbox_owner()],
            |row| row.get(0),
        )?)
    })
    .unwrap();
    assert_eq!(
        count, 1,
        "stable HTTP idempotency must admit exactly one task"
    );
}
