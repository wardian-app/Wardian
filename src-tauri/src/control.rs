use crate::state::AppState;
use std::fmt;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{
    AgentListResponse, AgentResponse, ControlRequest, OkResponse, WorkflowListResponse,
    WorkflowResponse, WorkflowSummary,
};
use wardian_core::identity::{normalize_status, AgentIdentity, StatusSource};

pub fn spawn_control_server(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_control_server(app).await {
            crate::utils::logging::log_debug(&format!(
                "[Wardian] control server unavailable: {error}"
            ));
        }
    });
}

#[cfg(windows)]
async fn run_control_server(app: AppHandle) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = wardian_core::control::pipe_name()
        .ok_or_else(|| std::io::Error::other("could not resolve Wardian control pipe"))?;

    let mut first_instance = true;
    loop {
        let server = ServerOptions::new()
            .first_pipe_instance(first_instance)
            .create(&pipe_name)?;
        first_instance = false;
        server.connect().await?;
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = handle_connection(server, app_handle).await {
                crate::utils::logging::log_debug(&format!(
                    "[Wardian] control request failed: {error}"
                ));
            }
        });
    }
}

#[cfg(unix)]
async fn run_control_server(app: AppHandle) -> std::io::Result<()> {
    use tokio::net::UnixListener;

    let socket_path = wardian_core::control::socket_path()
        .ok_or_else(|| std::io::Error::other("could not resolve Wardian control socket"))?;
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(socket_path)?;

    loop {
        let (stream, _) = listener.accept().await?;
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = handle_connection(stream, app_handle).await {
                crate::utils::logging::log_debug(&format!(
                    "[Wardian] control request failed: {error}"
                ));
            }
        });
    }
}

async fn handle_connection<T>(stream: T, app: AppHandle) -> std::io::Result<()>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let result = dispatch_request(&line, &app).await;

    let stream = reader.get_mut();
    match result {
        Ok(json) => {
            stream.write_all(json.as_bytes()).await?;
            stream.write_all(b"\n").await?;
            stream.flush().await?;
        }
        Err(error) => {
            let payload = error_payload(&error)?;
            stream.write_all(payload.as_bytes()).await?;
            stream.write_all(b"\n").await?;
            stream.flush().await?;
        }
    }

    Ok(())
}

async fn dispatch_request(line: &str, app: &AppHandle) -> Result<String, ControlError> {
    let req = serde_json::from_str::<ControlRequest>(line)
        .map_err(|e| ControlError::bad_request(format!("malformed control request JSON: {e}")))?;

    match req {
        ControlRequest::AgentList => {
            let response = AgentListResponse::new(live_agent_snapshots(app).await);
            ok_json(&response)
        }

        ControlRequest::AgentKill { target } => {
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            handle_agent_kill(app, uuid).await?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::AgentPause { target } => {
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            handle_agent_pause(app, &uuid).await?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::AgentResume { target } => {
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            crate::commands::agent::resume_agent(uuid, app.state::<AppState>(), app.clone())
                .await
                .map_err(ControlError::request_failed)?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::AgentSpawn {
            provider,
            class,
            name,
            workspace,
        } => {
            use crate::commands::agent::spawn_agent;
            let req = build_spawn_agent_request(provider, class, name, workspace);
            let config = spawn_agent(req, app.state::<AppState>(), app.clone())
                .await
                .map_err(ControlError::request_failed)?;
            let identity = agent_config_to_identity(&config, app).await;
            ok_json(&AgentResponse::new(identity))
        }

        ControlRequest::AgentClone { target, name } => {
            use crate::commands::agent::clone_agent;
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            let req = build_clone_agent_request(uuid, name);
            let config = clone_agent(req, app.state::<AppState>(), app.clone())
                .await
                .map_err(ControlError::request_failed)?;
            let identity = agent_config_to_identity(&config, app).await;
            ok_json(&AgentResponse::new(identity))
        }

        ControlRequest::WorkflowList => {
            let workflows = crate::workflow_engine::list_workflows().unwrap_or_default();
            ok_json(&WorkflowListResponse::new(workflow_summaries(&workflows)))
        }

        ControlRequest::WorkflowShow { target } => {
            let workflow = crate::workflow_engine::list_workflows()
                .unwrap_or_default()
                .into_iter()
                .find(|w| w.id == target || w.name == target)
                .ok_or_else(|| ControlError::not_found(format!("workflow not found: {target}")))?;
            ok_json(&WorkflowResponse::new(workflow))
        }

        ControlRequest::WorkflowRun { id } => {
            crate::workflow_engine::run_workflow(app.clone(), id, None)
                .await
                .map_err(ControlError::request_failed)?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::WorkflowStop { run_instance_id } => {
            crate::workflow_engine::stop_workflow_run(app.clone(), &run_instance_id).await;
            ok_json(&OkResponse::new())
        }

        ControlRequest::SendMessage {
            target,
            message,
            thread,
        } => {
            let state = app.state::<AppState>();
            deliver_message_to_target(&state, &target, &message, thread.as_deref()).await?;
            ok_json(&OkResponse::new())
        }
    }
}

fn build_spawn_agent_request(
    provider: String,
    class: String,
    name: Option<String>,
    workspace: Option<String>,
) -> crate::commands::agent::SpawnAgentRequest {
    let config_override = wardian_core::models::AgentConfig {
        provider,
        ..Default::default()
    };
    crate::commands::agent::SpawnAgentRequest {
        session_name: name.unwrap_or_default(),
        agent_class: class,
        folder: workspace.unwrap_or_default(),
        resume_session: None,
        is_off: None,
        config_override: Some(config_override),
    }
}

fn build_clone_agent_request(
    source_session_id: String,
    name: Option<String>,
) -> crate::commands::agent::CloneAgentRequest {
    crate::commands::agent::CloneAgentRequest {
        source_session_id,
        mode: crate::commands::agent::CloneAgentMode::Fresh,
        session_name: name,
        provider: None,
        folder: None,
        agent_class: None,
        start: Some(true),
    }
}

fn workflow_summaries(
    workflows: &[wardian_core::models::WorkflowDefinition],
) -> Vec<WorkflowSummary> {
    workflows
        .iter()
        .map(|w| WorkflowSummary {
            id: w.id.clone(),
            name: w.name.clone(),
            node_count: w.nodes.len(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Agent operation helpers
// ---------------------------------------------------------------------------

async fn handle_agent_kill(app: &AppHandle, session_id: String) -> std::io::Result<()> {
    let state = app.state::<AppState>();
    crate::commands::agent::kill_agent(session_id, state, app.clone())
        .await
        .map_err(std::io::Error::other)
}

async fn handle_agent_pause(app: &AppHandle, session_id: &str) -> std::io::Result<()> {
    let state = app.state::<AppState>();
    crate::commands::agent::pause_agent(session_id.to_string(), state, app.clone())
        .await
        .map_err(std::io::Error::other)
}

async fn resolve_target_uuid(app: &AppHandle, target: &str) -> Option<String> {
    let state = app.state::<AppState>();
    resolve_target_uuid_in_state(&state, target).await
}

async fn resolve_target_uuid_in_state(state: &AppState, target: &str) -> Option<String> {
    let agents = state.agents.lock().await;
    agents
        .iter()
        .find(|(id, agent)| {
            id.as_str() == target
                || agent
                    .config
                    .lock()
                    .map(|c| c.session_name == target)
                    .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
}

async fn resolve_send_targets_in_state(state: &AppState, target: &str) -> Vec<String> {
    let agents = state.agents.lock().await;

    if target == "all" {
        return agents.keys().cloned().collect();
    }

    if let Some(class) = target.strip_prefix("class:") {
        return agents
            .iter()
            .filter(|(_, a)| {
                a.config
                    .lock()
                    .map(|c| c.agent_class == class)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .collect();
    }

    agents
        .iter()
        .find(|(id, a)| {
            id.as_str() == target
                || a.config
                    .lock()
                    .map(|c| c.session_name == target)
                    .unwrap_or(false)
        })
        .map(|(id, _)| vec![id.clone()])
        .unwrap_or_default()
}

async fn deliver_message_to_target(
    state: &AppState,
    target: &str,
    message: &str,
    thread: Option<&str>,
) -> Result<(), ControlError> {
    let bytes = send_message_bytes(message, thread)?;
    let session_ids = resolve_send_targets_in_state(state, target).await;
    if session_ids.is_empty() {
        return Err(ControlError::not_found(format!(
            "no agents matched target: {target}"
        )));
    }
    let senders = state
        .input_senders
        .read()
        .map_err(|_| ControlError::request_failed("input_senders lock poisoned"))?;
    let mut delivered = 0usize;
    let mut failures = Vec::new();
    for session_id in &session_ids {
        match senders.get(session_id) {
            Some(tx) => match tx.try_send(bytes.clone()) {
                Ok(()) => delivered += 1,
                Err(error) => failures.push(format!("{session_id}: {error}")),
            },
            None => failures.push(format!("{session_id}: no input channel")),
        }
    }
    if delivered == 0 {
        return Err(ControlError::request_failed(format!(
            "message was not delivered to any matched agents: {}",
            failures.join("; ")
        )));
    }
    if !failures.is_empty() {
        return Err(ControlError::request_failed(format!(
            "message delivery failed for {} of {} matched agents: {}",
            failures.len(),
            session_ids.len(),
            failures.join("; ")
        )));
    }
    Ok(())
}

async fn agent_config_to_identity(
    config: &wardian_core::models::AgentConfig,
    app: &AppHandle,
) -> AgentIdentity {
    let state = app.state::<AppState>();
    let agents = state.agents.lock().await;
    if let Some(agent) = agents.get(&config.session_id) {
        snapshot_agent(agent)
    } else {
        AgentIdentity {
            name: config.session_name.clone(),
            uuid: config.session_id.clone(),
            class: config.agent_class.clone(),
            provider: config.provider.clone(),
            status: "idle".to_string(),
            pid: None,
            started_at: None,
            workspace: (!config.folder.trim().is_empty()).then_some(config.folder.clone()),
            last_status_at: None,
            status_source: StatusSource::Live,
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

fn ok_json<T: serde::Serialize>(value: &T) -> Result<String, ControlError> {
    serde_json::to_string(value).map_err(ControlError::request_failed)
}

fn error_payload(error: &ControlError) -> Result<String, std::io::Error> {
    serde_json::to_string(&serde_json::json!({
        "schema": wardian_core::control::CONTROL_SCHEMA,
        "error": {
            "code": error.code(),
            "message": error.to_string(),
        }
    }))
    .map_err(|e| std::io::Error::other(e.to_string()))
}

fn send_message_bytes(message: &str, thread: Option<&str>) -> Result<Vec<u8>, ControlError> {
    if thread.is_some() {
        return Err(ControlError::not_supported(
            "--thread is not supported by the Wardian control endpoint yet",
        ));
    }
    Ok(format!("{message}\r").into_bytes())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlError {
    code: &'static str,
    message: String,
}

impl ControlError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: "bad_request",
            message: message.into(),
        }
    }

    fn not_supported(message: impl Into<String>) -> Self {
        Self {
            code: "not_supported",
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "not_found",
            message: message.into(),
        }
    }

    fn request_failed(message: impl ToString) -> Self {
        Self {
            code: "request_failed",
            message: message.to_string(),
        }
    }

    fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ControlError {}

impl From<std::io::Error> for ControlError {
    fn from(error: std::io::Error) -> Self {
        Self::request_failed(error)
    }
}

// ---------------------------------------------------------------------------
// Agent snapshot (unchanged)
// ---------------------------------------------------------------------------

async fn live_agent_snapshots(app: &AppHandle) -> Vec<AgentIdentity> {
    let state = app.state::<AppState>();
    let agents = state.agents.lock().await;
    let order = state.agent_order.lock().await.clone();
    let mut snapshots = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for session_id in order {
        if let Some(agent) = agents.get(&session_id) {
            snapshots.push(snapshot_agent(agent));
            seen.insert(session_id);
        }
    }

    for (session_id, agent) in agents.iter() {
        if !seen.contains(session_id) {
            snapshots.push(snapshot_agent(agent));
        }
    }

    snapshots
}

fn snapshot_agent(agent: &crate::state::ActiveAgent) -> AgentIdentity {
    let config = agent
        .config
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let status = agent
        .current_status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let started_at = agent
        .init_timestamp
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();

    AgentIdentity {
        name: config.session_name,
        uuid: config.session_id,
        class: config.agent_class,
        provider: config.provider,
        status: normalize_status(&status),
        pid: agent.process_id,
        started_at,
        workspace: (!config.folder.trim().is_empty()).then_some(config.folder),
        last_status_at: None,
        status_source: StatusSource::Live,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ActiveAgent;
    use std::sync::{Arc, Mutex};
    use wardian_core::models::{AgentConfig, WorkflowDefinition, WorkflowNode, WorkflowSettings};

    fn test_agent(session_id: &str, session_name: &str, agent_class: &str) -> ActiveAgent {
        ActiveAgent {
            config: Arc::new(Mutex::new(AgentConfig {
                session_id: session_id.to_string(),
                session_name: session_name.to_string(),
                agent_class: agent_class.to_string(),
                provider: "mock".to_string(),
                folder: "D:/work".to_string(),
                ..Default::default()
            })),
            child_process: None,
            background_processes: Vec::new(),
            pty_master: None,
            stdin_tx: None,
            output_buffer: Arc::new(Mutex::new(String::new())),
            process_id: Some(1234),
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(Some("2026-05-07T00:00:00.000Z".to_string()))),
            current_status: Arc::new(Mutex::new("Processing".to_string())),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    async fn insert_test_agent(
        state: &AppState,
        session_id: &str,
        session_name: &str,
        agent_class: &str,
    ) {
        state.agents.lock().await.insert(
            session_id.to_string(),
            test_agent(session_id, session_name, agent_class),
        );
    }

    fn sample_workflow(id: &str, name: &str, nodes: Vec<WorkflowNode>) -> WorkflowDefinition {
        WorkflowDefinition {
            id: id.to_string(),
            name: name.to_string(),
            settings: WorkflowSettings {
                max_iterations: 10,
                on_limit_reached: "stop".to_string(),
            },
            nodes,
            role_mappings: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn parse_errors_emit_bad_request_code() {
        let error = ControlError::bad_request("expected value");
        let payload = error_payload(&error).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(value["error"]["code"], "bad_request");
        assert_eq!(value["schema"], wardian_core::control::CONTROL_SCHEMA);
    }

    #[test]
    fn send_message_rejects_thread_until_supported() {
        let error = send_message_bytes("hello", Some("review")).unwrap_err();

        assert_eq!(error.code(), "not_supported");
        assert!(error.to_string().contains("--thread is not supported"));
    }

    #[test]
    fn send_message_submits_with_terminal_enter() {
        assert_eq!(send_message_bytes("hello", None).unwrap(), b"hello\r");
    }

    #[test]
    fn not_found_errors_emit_not_found_code() {
        let error = ControlError::not_found("agent not found: ghost");
        let payload = error_payload(&error).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(value["error"]["code"], "not_found");
        assert!(value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ghost"));
    }

    #[test]
    fn spawn_request_preserves_provider_and_defaults_optional_fields() {
        let req =
            build_spawn_agent_request("codex".to_string(), "Reviewer".to_string(), None, None);

        assert_eq!(req.session_name, "");
        assert_eq!(req.agent_class, "Reviewer");
        assert_eq!(req.folder, "");
        assert_eq!(req.resume_session, None);
        assert_eq!(
            req.config_override
                .as_ref()
                .map(|config| config.provider.as_str()),
            Some("codex")
        );
    }

    #[test]
    fn clone_request_uses_fresh_started_clone_by_default() {
        let req = build_clone_agent_request("source-1".to_string(), Some("reviewer-2".into()));

        assert_eq!(req.source_session_id, "source-1");
        assert_eq!(req.mode, crate::commands::agent::CloneAgentMode::Fresh);
        assert_eq!(req.session_name.as_deref(), Some("reviewer-2"));
        assert_eq!(req.provider, None);
        assert_eq!(req.folder, None);
        assert_eq!(req.agent_class, None);
        assert_eq!(req.start, Some(true));
    }

    #[test]
    fn workflow_summaries_count_nodes_without_serializing_full_workflows() {
        let summaries = workflow_summaries(&[
            sample_workflow(
                "wf-a",
                "Daily review",
                vec![WorkflowNode {
                    id: "n1".to_string(),
                    r#type: "agent".to_string(),
                    name: Some("Reviewer".to_string()),
                    config: serde_json::json!({}),
                    parameter_schema: None,
                    dependencies: None,
                    position: None,
                }],
            ),
            sample_workflow("wf-b", "Empty", Vec::new()),
        ]);

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, "wf-a");
        assert_eq!(summaries[0].node_count, 1);
        assert_eq!(summaries[1].node_count, 0);
    }

    #[tokio::test]
    async fn target_resolution_matches_uuid_or_session_name() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;

        assert_eq!(
            resolve_target_uuid_in_state(&state, "agent-1")
                .await
                .as_deref(),
            Some("agent-1")
        );
        assert_eq!(
            resolve_target_uuid_in_state(&state, "CoderOne")
                .await
                .as_deref(),
            Some("agent-1")
        );
        assert_eq!(resolve_target_uuid_in_state(&state, "missing").await, None);
    }

    #[tokio::test]
    async fn send_target_resolution_supports_all_class_uuid_and_name() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        insert_test_agent(&state, "agent-2", "ReviewerOne", "Reviewer").await;

        let mut all = resolve_send_targets_in_state(&state, "all").await;
        all.sort();
        assert_eq!(all, vec!["agent-1".to_string(), "agent-2".to_string()]);

        assert_eq!(
            resolve_send_targets_in_state(&state, "class:Reviewer").await,
            vec!["agent-2".to_string()]
        );
        assert_eq!(
            resolve_send_targets_in_state(&state, "CoderOne").await,
            vec!["agent-1".to_string()]
        );
        assert_eq!(
            resolve_send_targets_in_state(&state, "agent-2").await,
            vec!["agent-2".to_string()]
        );
        assert!(resolve_send_targets_in_state(&state, "class:Missing")
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn message_delivery_writes_terminal_bytes_to_matched_agent() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        deliver_message_to_target(&state, "CoderOne", "hello", None)
            .await
            .unwrap();

        assert_eq!(rx.try_recv().unwrap(), b"hello\r");
    }

    #[tokio::test]
    async fn message_delivery_reports_missing_target_as_not_found() {
        let state = AppState::new();

        let error = deliver_message_to_target(&state, "ghost", "hello", None)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "not_found");
        assert!(error
            .to_string()
            .contains("no agents matched target: ghost"));
    }

    #[tokio::test]
    async fn message_delivery_reports_agent_without_input_channel() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;

        let error = deliver_message_to_target(&state, "agent-1", "hello", None)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "request_failed");
        assert!(error.to_string().contains("agent-1: no input channel"));
    }

    #[tokio::test]
    async fn message_delivery_reports_partial_failures_after_successful_delivery() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        insert_test_agent(&state, "agent-2", "CoderTwo", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let error = deliver_message_to_target(&state, "class:Coder", "hello", None)
            .await
            .unwrap_err();

        assert_eq!(rx.try_recv().unwrap(), b"hello\r");
        assert_eq!(error.code(), "request_failed");
        assert!(error
            .to_string()
            .contains("message delivery failed for 1 of 2 matched agents"));
        assert!(error.to_string().contains("agent-2: no input channel"));
    }

    #[test]
    fn snapshot_agent_normalizes_status_and_omits_blank_workspace() {
        let agent = test_agent("agent-1", "CoderOne", "Coder");
        {
            let mut config = agent.config.lock().unwrap();
            config.folder.clear();
        }

        let snapshot = snapshot_agent(&agent);

        assert_eq!(snapshot.uuid, "agent-1");
        assert_eq!(snapshot.name, "CoderOne");
        assert_eq!(snapshot.class, "Coder");
        assert_eq!(snapshot.provider, "mock");
        assert_eq!(snapshot.status, "processing");
        assert_eq!(snapshot.pid, Some(1234));
        assert_eq!(
            snapshot.started_at.as_deref(),
            Some("2026-05-07T00:00:00.000Z")
        );
        assert_eq!(snapshot.workspace, None);
        assert_eq!(snapshot.status_source, StatusSource::Live);
    }
}
