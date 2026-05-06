use crate::state::AppState;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{
    AgentListResponse, AgentResponse, ControlRequest, OkResponse, WorkflowListResponse,
    WorkflowSummary,
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
            let payload = serde_json::json!({
                "schema": wardian_core::control::CONTROL_SCHEMA,
                "error": {
                    "code": "request_failed",
                    "message": error.to_string(),
                }
            })
            .to_string();
            stream.write_all(payload.as_bytes()).await?;
            stream.write_all(b"\n").await?;
            stream.flush().await?;
        }
    }

    Ok(())
}

async fn dispatch_request(line: &str, app: &AppHandle) -> Result<String, std::io::Error> {
    let req = serde_json::from_str::<ControlRequest>(line).map_err(|e| {
        std::io::Error::other(format!("bad_request: {e}"))
    })?;

    match req {
        ControlRequest::AgentList => {
            let response = AgentListResponse::new(live_agent_snapshots(app).await);
            ok_json(&response)
        }

        ControlRequest::AgentKill { target } => {
            let uuid = resolve_target_uuid(app, &target).await
                .ok_or_else(|| std::io::Error::other(format!("agent not found: {target}")))?;
            handle_agent_kill(app, uuid).await?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::AgentPause { target } => {
            let uuid = resolve_target_uuid(app, &target).await
                .ok_or_else(|| std::io::Error::other(format!("agent not found: {target}")))?;
            handle_agent_pause(app, &uuid).await?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::AgentResume { target } => {
            let uuid = resolve_target_uuid(app, &target).await
                .ok_or_else(|| std::io::Error::other(format!("agent not found: {target}")))?;
            crate::commands::agent::resume_agent(uuid, app.state::<AppState>(), app.clone())
                .await
                .map_err(std::io::Error::other)?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::AgentSpawn { class, name, workspace } => {
            use crate::commands::agent::{spawn_agent, SpawnAgentRequest};
            let req = SpawnAgentRequest {
                session_name: name.unwrap_or_default(),
                agent_class: class,
                folder: workspace.unwrap_or_default(),
                resume_session: None,
                is_off: None,
                config_override: None,
            };
            let config = spawn_agent(req, app.state::<AppState>(), app.clone())
                .await
                .map_err(std::io::Error::other)?;
            let identity = agent_config_to_identity(&config, app).await;
            ok_json(&AgentResponse::new(identity))
        }

        ControlRequest::AgentClone { target, name } => {
            use crate::commands::agent::{clone_agent, CloneAgentMode, CloneAgentRequest};
            let uuid = resolve_target_uuid(app, &target).await
                .ok_or_else(|| std::io::Error::other(format!("agent not found: {target}")))?;
            let req = CloneAgentRequest {
                source_session_id: uuid,
                mode: CloneAgentMode::Fresh,
                session_name: name,
                provider: None,
                folder: None,
                agent_class: None,
                start: Some(true),
            };
            let config = clone_agent(req, app.state::<AppState>(), app.clone())
                .await
                .map_err(std::io::Error::other)?;
            let identity = agent_config_to_identity(&config, app).await;
            ok_json(&AgentResponse::new(identity))
        }

        ControlRequest::WorkflowList => {
            let workflows = crate::workflow_engine::list_workflows().unwrap_or_default();
            let summaries: Vec<WorkflowSummary> = workflows
                .iter()
                .map(|w| WorkflowSummary {
                    id: w.id.clone(),
                    name: w.name.clone(),
                    node_count: w.nodes.len(),
                })
                .collect();
            ok_json(&WorkflowListResponse::new(summaries))
        }

        ControlRequest::WorkflowRun { id } => {
            crate::workflow_engine::run_workflow(app.clone(), id, None)
                .await
                .map_err(std::io::Error::other)?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::WorkflowStop { run_instance_id } => {
            crate::workflow_engine::stop_workflow_run(app.clone(), &run_instance_id).await;
            ok_json(&OkResponse::new())
        }

        ControlRequest::SendMessage { target, message, thread: _ } => {
            let session_ids = resolve_send_targets(app, &target).await;
            if session_ids.is_empty() {
                return Err(std::io::Error::other(format!(
                    "no agents matched target: {target}"
                )));
            }
            let state = app.state::<AppState>();
            let senders = state
                .input_senders
                .read()
                .map_err(|_| std::io::Error::other("input_senders lock poisoned"))?;
            let bytes = format!("{message}\n").into_bytes();
            for session_id in &session_ids {
                if let Some(tx) = senders.get(session_id) {
                    let _ = tx.try_send(bytes.clone());
                }
            }
            ok_json(&OkResponse::new())
        }
    }
}

// ---------------------------------------------------------------------------
// Agent operation helpers
// ---------------------------------------------------------------------------

async fn handle_agent_kill(app: &AppHandle, session_id: String) -> std::io::Result<()> {
    let state = app.state::<AppState>();
    let mut agents = state.agents.lock().await;
    let mut order = state.agent_order.lock().await;

    if let Some(mut agent) = agents.remove(&session_id) {
        if let Ok(mut senders) = state.input_senders.write() {
            senders.remove(&session_id);
        }
        order.retain(|id| id != &session_id);
        crate::manager::save_state(app, &agents, &order);
        crate::manager::terminate_active_agent_process(&mut agent);
        let _ = wardian_core::db::delete_agent(&session_id);
        if let Some(home) = crate::utils::fs::get_wardian_home() {
            let agent_dir = home.join("agents").join(&session_id);
            if agent_dir.exists() {
                let _ = std::fs::remove_dir_all(&agent_dir);
            }
        }
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "agent not found: {session_id}"
        )))
    }
}

async fn handle_agent_pause(app: &AppHandle, session_id: &str) -> std::io::Result<()> {
    let state = app.state::<AppState>();
    crate::commands::agent::pause_agent(
        session_id.to_string(),
        state,
        app.clone(),
    )
    .await
    .map_err(std::io::Error::other)
}

async fn resolve_target_uuid(app: &AppHandle, target: &str) -> Option<String> {
    let state = app.state::<AppState>();
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

async fn resolve_send_targets(app: &AppHandle, target: &str) -> Vec<String> {
    let state = app.state::<AppState>();
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

fn ok_json<T: serde::Serialize>(value: &T) -> Result<String, std::io::Error> {
    serde_json::to_string(value).map_err(|e| std::io::Error::other(e.to_string()))
}

// ---------------------------------------------------------------------------
// Agent snapshot (unchanged)
// ---------------------------------------------------------------------------

async fn live_agent_snapshots(app: &AppHandle) -> Vec<AgentIdentity> {
    let state = app.state::<AppState>();
    let order = state.agent_order.lock().await.clone();
    let agents = state.agents.lock().await;
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
