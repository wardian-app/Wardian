use crate::state::AppState;
use std::{
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{
    AgentListResponse, AgentResponse, AgentWatchResponse, ControlRequest, DeliveryDetail,
    DeliveryErrorDetail, MessageOrigin, OkResponse, SendMessageResponse, WatchAgentSnapshot,
    WatchDeliverySnapshot, WorkflowListResponse, WorkflowResponse, WorkflowSummary,
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
            origin,
        } => {
            let state = app.state::<AppState>();
            let delivery = deliver_message_to_target(
                Some(app),
                &state,
                &target,
                &message,
                thread.as_deref(),
                origin.as_ref(),
            )
            .await?;
            ok_json(&SendMessageResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                ok: true,
                delivery,
            })
        }

        ControlRequest::AgentWatch {
            target,
            since,
            until,
            include: _,
            tail_bytes,
            follow,
            timeout_ms,
        } => handle_agent_watch(app, &target, since, until, tail_bytes, follow, timeout_ms).await,
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
        profile_selection: None,
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
    app: Option<&AppHandle>,
    state: &AppState,
    target: &str,
    message: &str,
    thread: Option<&str>,
    origin: Option<&MessageOrigin>,
) -> Result<Vec<DeliveryDetail>, ControlError> {
    validate_send_message_thread(thread)?;
    let session_ids = resolve_send_targets_in_state(state, target).await;
    if session_ids.is_empty() {
        return Err(ControlError::not_found(format!(
            "no agents matched target: {target}"
        )));
    }

    let target_infos = delivery_target_infos(state, &session_ids).await?;
    let senders = {
        let senders = state
            .input_senders
            .read()
            .map_err(|_| ControlError::request_failed("input_senders lock poisoned"))?;
        session_ids
            .iter()
            .map(|session_id| (session_id.clone(), senders.get(session_id).cloned()))
            .collect::<std::collections::HashMap<_, _>>()
    };

    let mut delivered = 0usize;
    let mut delivered_session_ids = Vec::new();
    let mut failures = Vec::new();
    let mut delivery = Vec::with_capacity(session_ids.len());
    for info in target_infos {
        match senders.get(&info.uuid).and_then(Clone::clone) {
            Some(tx) => {
                let outbound_message =
                    message_with_origin(state, message, origin, info.status == "action_required")
                        .await;
                let result = match wait_for_terminal_ready_for_control_send(state, &info).await {
                    Ok(()) => {
                        crate::utils::terminal_input::submit_prompt_chunks_via_sender(
                            &tx,
                            &info.provider,
                            &outbound_message,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                match result {
                    Ok(()) => {
                        delivered += 1;
                        let detail = DeliveryDetail {
                            uuid: info.uuid,
                            name: info.name,
                            provider: info.provider,
                            runtime_state: "live_pty_available".to_string(),
                            delivery_state: "submitted".to_string(),
                            error: None,
                        };
                        delivered_session_ids.push(detail.uuid.clone());
                        record_delivery_attempt(state, &detail).await;
                        delivery.push(detail);
                    }
                    Err(error) => {
                        failures.push(format!("{}: {error}", info.uuid));
                        let detail = failed_delivery_detail(
                            info,
                            "live_pty_available",
                            "send_failed",
                            error,
                        );
                        record_delivery_attempt(state, &detail).await;
                        delivery.push(detail);
                    }
                }
            }
            None => {
                failures.push(format!("{}: no input channel", info.uuid));
                let runtime_state = if info.status == "off" {
                    "target_off"
                } else {
                    "restored_without_sender"
                };
                let detail = failed_delivery_detail(
                    info,
                    runtime_state,
                    "no_input_channel",
                    "missing sender",
                );
                record_delivery_attempt(state, &detail).await;
                delivery.push(detail);
            }
        }
    }
    mark_delivered_agents_prompt_started(app, state, &delivered_session_ids).await;

    if delivered == 0 {
        return Err(ControlError::request_failed(format!(
            "message was not delivered to any matched agents: {}",
            failures.join("; ")
        ))
        .with_details(delivery_details_json(&delivery)));
    }
    if !failures.is_empty() {
        return Err(ControlError::request_failed(format!(
            "message delivery failed for {} of {} matched agents: {}",
            failures.len(),
            session_ids.len(),
            failures.join("; ")
        ))
        .with_details(delivery_details_json(&delivery)));
    }
    Ok(delivery)
}

async fn message_with_origin(
    state: &AppState,
    message: &str,
    origin: Option<&MessageOrigin>,
    allow_bare_approval_response: bool,
) -> String {
    if allow_bare_approval_response && is_bare_approval_response(message) {
        return message.to_string();
    }

    let Some(MessageOrigin::WardianAgent { session_id }) = origin else {
        return message.to_string();
    };

    match resolve_agent_name_in_state(state, session_id).await {
        Some(name) => format!("From {name}: {message}"),
        None => format!("From Wardian agent {session_id}: {message}"),
    }
}

fn is_bare_approval_response(message: &str) -> bool {
    matches!(
        message.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "n" | "no"
    )
}

async fn resolve_agent_name_in_state(state: &AppState, session_id: &str) -> Option<String> {
    let agents = state.agents.lock().await;
    agents.get(session_id).and_then(|agent| {
        agent
            .config
            .lock()
            .map(|config| config.session_name.clone())
            .ok()
    })
}

async fn wait_for_terminal_ready_for_control_send(
    state: &AppState,
    info: &DeliveryTargetInfo,
) -> Result<(), String> {
    if info.provider == "opencode" {
        wait_for_opencode_terminal_ready(state, &info.uuid, 15_000).await
    } else if info.provider == "codex" {
        wait_for_terminal_output(state, &info.uuid, 15_000, codex_output_has_ready_prompt).await
    } else {
        Ok(())
    }
}

async fn wait_for_opencode_terminal_ready(
    state: &AppState,
    session_id: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(timeout_ms) {
        let (title, status) = {
            let agents = state.agents.lock().await;
            let agent = agents
                .get(session_id)
                .ok_or_else(|| format!("Agent {} not found or is off", session_id))?;
            let title = agent
                .terminal_title
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            let status = agent
                .current_status
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            (title, status)
        };
        let title = title.trim();
        if wardian_core::identity::normalize_status(&status) == "idle"
            && (title == "OpenCode" || title.starts_with("OC | "))
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(format!(
        "Timed out waiting for {} OpenCode terminal to become ready",
        session_id
    ))
}

async fn wait_for_terminal_output(
    state: &AppState,
    session_id: &str,
    timeout_ms: u64,
    is_ready: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(timeout_ms) {
        let watch_state = {
            let agents = state.agents.lock().await;
            agents
                .get(session_id)
                .ok_or_else(|| format!("Agent {} not found or is off", session_id))?
                .watch_state
                .clone()
        };
        let output = watch_state
            .lock()
            .map_err(|_| format!("Agent {} watch state lock poisoned", session_id))?
            .snapshot_since(None, None)
            .map(|snapshot| snapshot.output.text)
            .unwrap_or_default();
        if is_ready(&output) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(format!(
        "Timed out waiting for {} terminal output to become ready",
        session_id
    ))
}

fn codex_output_has_ready_prompt(output: &str) -> bool {
    output.contains("\n›") || output.contains("\r\n›")
}

async fn mark_delivered_agents_prompt_started(
    app: Option<&AppHandle>,
    state: &AppState,
    session_ids: &[String],
) {
    if session_ids.is_empty() {
        return;
    }

    let agents = state.agents.lock().await;
    for session_id in session_ids {
        if let Some(agent) = agents.get(session_id) {
            if crate::manager::mark_agent_prompt_started(agent) {
                if let Some(app) = app {
                    crate::manager::set_agent_status(
                        app,
                        session_id,
                        &agent.current_status,
                        "Processing...",
                    );
                }
            }
        }
    }
}

async fn handle_agent_watch(
    app: &AppHandle,
    target: &str,
    since: Option<String>,
    until: Option<String>,
    tail_bytes: Option<usize>,
    follow: bool,
    timeout_ms: Option<u64>,
) -> Result<String, ControlError> {
    validate_watch_follow(follow)?;
    validate_watch_target(target)?;
    let condition = until.as_deref().map(parse_watch_condition).transpose()?;
    let state = app.state::<AppState>();
    let uuid = resolve_target_uuid_in_state(&state, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let watch_state = agent_watch_state(&state, &uuid).await?;
    let snapshot = if let Some(condition) = condition {
        wait_for_watch_condition(
            watch_state,
            since,
            condition,
            Duration::from_millis(timeout_ms.unwrap_or(30_000)),
            tail_bytes,
        )
        .await?
    } else {
        watch_state
            .lock()
            .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?
            .snapshot_since(since.as_deref(), tail_bytes)
            .map_err(control_error_from_watch_state)?
    };
    let agent = watch_agent_snapshot(&state, &uuid).await?;
    let delivery = delivery_snapshot_from_events(&snapshot.events);

    ok_json(&AgentWatchResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        agent,
        cursor: snapshot.cursor,
        events: snapshot.events,
        output: snapshot.output,
        delivery,
    })
}

fn validate_watch_target(target: &str) -> Result<(), ControlError> {
    if target == "all" || target.starts_with("class:") {
        return Err(ControlError::not_supported(
            "agent watch requires a single agent name or uuid",
        ));
    }
    Ok(())
}

fn validate_watch_follow(follow: bool) -> Result<(), ControlError> {
    if follow {
        return Err(ControlError::not_supported(
            "agent watch --follow is reserved for a future streaming implementation",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WatchCondition {
    Status(String),
    OutputContains(String),
    EventKind(String),
    DeliveryState(String),
}

fn parse_watch_condition(value: &str) -> Result<WatchCondition, ControlError> {
    let Some((kind, argument)) = value.split_once(':') else {
        return Err(ControlError::not_supported(format!(
            "unsupported watch condition: {value}"
        )));
    };
    match kind {
        "status" => Ok(WatchCondition::Status(normalize_status(argument))),
        "output" => Ok(WatchCondition::OutputContains(argument.to_string())),
        "event" => Ok(WatchCondition::EventKind(argument.to_string())),
        "delivery" => Ok(WatchCondition::DeliveryState(argument.to_string())),
        _ => Err(ControlError::not_supported(format!(
            "unsupported watch condition: {value}"
        ))),
    }
}

async fn wait_for_watch_condition(
    state: Arc<Mutex<crate::state::AgentWatchState>>,
    since: Option<String>,
    condition: WatchCondition,
    timeout: Duration,
    tail_bytes: Option<usize>,
) -> Result<crate::state::agent_watch::WatchSnapshot, ControlError> {
    let started = std::time::Instant::now();
    let notify = state
        .lock()
        .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?
        .notifier();

    loop {
        let notified = notify.notified();
        let snapshot = {
            let guard = state
                .lock()
                .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?;
            guard.snapshot_since(since.as_deref(), tail_bytes)
        };

        match snapshot {
            Ok(snapshot) if watch_condition_matches(&condition, &snapshot) => return Ok(snapshot),
            Ok(_) => {}
            Err(error) if error.code() == "cursor_expired" => {
                return Err(
                    ControlError::gap_detected("watch cursor expired while waiting")
                        .with_details(error.details().clone()),
                );
            }
            Err(error) => return Err(control_error_from_watch_state(error)),
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(ControlError::watch_timeout("watch condition timed out"));
        }
        let remaining = timeout - elapsed;
        if tokio::time::timeout(remaining, notified).await.is_err() {
            return Err(ControlError::watch_timeout("watch condition timed out"));
        }
    }
}

fn watch_condition_matches(
    condition: &WatchCondition,
    snapshot: &crate::state::agent_watch::WatchSnapshot,
) -> bool {
    match condition {
        WatchCondition::Status(status) => snapshot.events.iter().any(|event| {
            event.kind == "status"
                && event
                    .payload
                    .get("status")
                    .and_then(|value| value.as_str())
                    .is_some_and(|value| normalize_status(value) == *status)
        }),
        WatchCondition::OutputContains(token) => snapshot.output.text.contains(token),
        WatchCondition::EventKind(kind) => snapshot.events.iter().any(|event| &event.kind == kind),
        WatchCondition::DeliveryState(state) => snapshot.events.iter().any(|event| {
            event.kind == "delivery"
                && event
                    .payload
                    .get("delivery_state")
                    .and_then(|value| value.as_str())
                    == Some(state.as_str())
        }),
    }
}

fn control_error_from_watch_state(
    error: crate::state::agent_watch::WatchStateError,
) -> ControlError {
    ControlError::coded(error.code(), "watch state error").with_details(error.details().clone())
}

async fn agent_watch_state(
    state: &AppState,
    uuid: &str,
) -> Result<Arc<Mutex<crate::state::AgentWatchState>>, ControlError> {
    let agents = state.agents.lock().await;
    agents
        .get(uuid)
        .map(|agent| agent.watch_state.clone())
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {uuid}")))
}

async fn watch_agent_snapshot(
    state: &AppState,
    uuid: &str,
) -> Result<WatchAgentSnapshot, ControlError> {
    let agents = state.agents.lock().await;
    let agent = agents
        .get(uuid)
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {uuid}")))?;
    let config = agent
        .config
        .lock()
        .map_err(|_| ControlError::request_failed("agent config lock poisoned"))?;
    let status = agent
        .current_status
        .lock()
        .map_err(|_| ControlError::request_failed("agent status lock poisoned"))?;
    let last_status_at = agent
        .last_status_at
        .lock()
        .map_err(|_| ControlError::request_failed("agent status timestamp lock poisoned"))?
        .clone();
    Ok(WatchAgentSnapshot {
        uuid: uuid.to_string(),
        name: config.session_name.clone(),
        provider: config.provider.clone(),
        status: normalize_status(&status),
        last_status_at,
    })
}

fn delivery_snapshot_from_events(
    events: &[wardian_core::control::WatchEvent],
) -> WatchDeliverySnapshot {
    let delivery = events
        .iter()
        .filter(|event| event.kind == "delivery")
        .filter_map(|event| serde_json::from_value::<DeliveryDetail>(event.payload.clone()).ok())
        .collect();
    WatchDeliverySnapshot { delivery }
}

fn validate_send_message_thread(thread: Option<&str>) -> Result<(), ControlError> {
    if thread.is_some() {
        return Err(ControlError::not_supported(
            "--thread is not supported by the Wardian control endpoint yet",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct DeliveryTargetInfo {
    uuid: String,
    name: String,
    provider: String,
    status: String,
}

async fn delivery_target_infos(
    state: &AppState,
    session_ids: &[String],
) -> Result<Vec<DeliveryTargetInfo>, ControlError> {
    let agents = state.agents.lock().await;
    session_ids
        .iter()
        .map(|session_id| {
            let agent = agents.get(session_id).ok_or_else(|| {
                ControlError::not_found(format!("agent not found after resolution: {session_id}"))
            })?;
            let config = agent
                .config
                .lock()
                .map_err(|_| ControlError::request_failed("agent config lock poisoned"))?;
            let status = agent
                .current_status
                .lock()
                .map_err(|_| ControlError::request_failed("agent status lock poisoned"))?;
            Ok(DeliveryTargetInfo {
                uuid: session_id.clone(),
                name: config.session_name.clone(),
                provider: config.provider.clone(),
                status: normalize_status(&status),
            })
        })
        .collect()
}

fn failed_delivery_detail(
    info: DeliveryTargetInfo,
    runtime_state: &str,
    error_code: &str,
    error_message: impl Into<String>,
) -> DeliveryDetail {
    DeliveryDetail {
        uuid: info.uuid,
        name: info.name,
        provider: info.provider,
        runtime_state: runtime_state.to_string(),
        delivery_state: "failed".to_string(),
        error: Some(DeliveryErrorDetail {
            code: error_code.to_string(),
            message: error_message.into(),
        }),
    }
}

fn delivery_details_json(delivery: &[DeliveryDetail]) -> serde_json::Value {
    serde_json::json!({ "delivery": delivery })
}

async fn record_delivery_attempt(state: &AppState, detail: &DeliveryDetail) {
    let agents = state.agents.lock().await;
    if let Some(agent) = agents.get(&detail.uuid) {
        if let Ok(mut watch_state) = agent.watch_state.lock() {
            watch_state.push_delivery(serde_json::json!(detail));
        }
    }
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
    let mut error_body = serde_json::json!({
        "code": error.code(),
        "message": error.to_string(),
    });
    if let Some(details) = error.details() {
        error_body["details"] = details.clone();
    }

    serde_json::to_string(&serde_json::json!({
        "schema": wardian_core::control::CONTROL_SCHEMA,
        "error": error_body
    }))
    .map_err(|e| std::io::Error::other(e.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlError {
    code: &'static str,
    message: String,
    details: Option<serde_json::Value>,
}

impl ControlError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: "bad_request",
            message: message.into(),
            details: None,
        }
    }

    fn not_supported(message: impl Into<String>) -> Self {
        Self {
            code: "not_supported",
            message: message.into(),
            details: None,
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "not_found",
            message: message.into(),
            details: None,
        }
    }

    fn request_failed(message: impl ToString) -> Self {
        Self {
            code: "request_failed",
            message: message.to_string(),
            details: None,
        }
    }

    fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    fn watch_timeout(message: impl Into<String>) -> Self {
        Self::coded("watch_timeout", message)
    }

    fn gap_detected(message: impl Into<String>) -> Self {
        Self::coded("gap_detected", message)
    }

    fn code(&self) -> &'static str {
        self.code
    }

    fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    fn details(&self) -> Option<&serde_json::Value> {
        self.details.as_ref()
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
    let last_status_at = agent
        .last_status_at
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
        last_status_at,
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
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
                session_id.to_string(),
                4096,
                262_144,
            ))),
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
        let error = validate_send_message_thread(Some("review")).unwrap_err();

        assert_eq!(error.code(), "not_supported");
        assert!(error.to_string().contains("--thread is not supported"));
    }

    #[test]
    fn send_message_without_thread_is_valid() {
        validate_send_message_thread(None).unwrap();
    }

    #[test]
    fn control_send_uses_codex_submit_sequence() {
        let chunks =
            crate::utils::terminal_input::provider_submit_chunks("codex", "hello\nworld").unwrap();

        assert_eq!(chunks[0], b"hello world".to_vec());
        assert_eq!(chunks[1], b"\r".to_vec());
    }

    #[test]
    fn control_send_uses_plain_enter_for_gemini_and_claude() {
        let gemini =
            crate::utils::terminal_input::provider_submit_chunks("gemini", "hello").unwrap();
        let claude =
            crate::utils::terminal_input::provider_submit_chunks("claude", "hello").unwrap();

        assert_eq!(gemini, vec![b"hello".to_vec(), b"\r".to_vec()]);
        assert_eq!(claude, vec![b"hello".to_vec(), b"\r".to_vec()]);
    }

    #[test]
    fn codex_ready_prompt_detects_visible_compose_prompt() {
        assert!(codex_output_has_ready_prompt(
            "\r\n› Write tests for @filename"
        ));
        assert!(codex_output_has_ready_prompt(
            "\r\n›\u{1b}[22m Write tests for @filename"
        ));
        assert!(!codex_output_has_ready_prompt("Booting MCP server"));
    }

    #[tokio::test]
    async fn opencode_control_send_waits_for_open_code_title() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "OpenCodeOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "opencode".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
            *agent.terminal_title.lock().unwrap() = "OpenCode".to_string();
        }
        let info = delivery_target_infos(&state, &["agent-1".to_string()])
            .await
            .unwrap()
            .remove(0);

        wait_for_terminal_ready_for_control_send(&state, &info)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn opencode_control_send_accepts_idle_oc_title() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "OpenCodeOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "opencode".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
            *agent.terminal_title.lock().unwrap() = "OC | Self-introduction".to_string();
        }
        let info = delivery_target_infos(&state, &["agent-1".to_string()])
            .await
            .unwrap()
            .remove(0);

        wait_for_terminal_ready_for_control_send(&state, &info)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn message_delivery_writes_terminal_bytes_after_opencode_is_ready() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "OpenCodeOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "opencode".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
            *agent.terminal_title.lock().unwrap() = "OpenCode".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        deliver_message_to_target(None, &state, "OpenCodeOne", "hello", None, None)
            .await
            .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
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
    fn error_payload_serializes_delivery_details() {
        let error = ControlError::request_failed("message delivery failed").with_details(
            serde_json::json!({
                "delivery": [{
                    "uuid": "agent-2",
                    "name": "CoderTwo",
                    "provider": "claude",
                    "runtime_state": "restored_without_sender",
                    "delivery_state": "failed",
                    "error": {
                        "code": "no_input_channel",
                        "message": "missing sender"
                    }
                }]
            }),
        );

        let payload = error_payload(&error).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(
            value["error"]["details"]["delivery"][0]["runtime_state"],
            "restored_without_sender"
        );
        assert_eq!(
            value["error"]["details"]["delivery"][0]["error"]["code"],
            "no_input_channel"
        );
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
        assert!(req.profile_selection.is_none());
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
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
            *agent.query_count.lock().unwrap() = 0;
            agent
                .watch_state
                .lock()
                .unwrap()
                .push_output(b"\r\n\x1b[1m\r\n\xe2\x80\xba\x1b[22m Write tests for @filename");
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        deliver_message_to_target(None, &state, "CoderOne", "hello", None, None)
            .await
            .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            assert_eq!(agent.current_status.lock().unwrap().as_str(), "Idle");
            assert_eq!(*agent.query_count.lock().unwrap(), 1);
        }
    }

    #[tokio::test]
    async fn message_delivery_prefixes_agent_origin_with_sender_name() {
        let state = AppState::new();
        insert_test_agent(&state, "source-1", "PlannerOne", "Planner").await;
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "check this",
            None,
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            rx.recv().await.unwrap(),
            b"From PlannerOne: check this".to_vec()
        );
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
    }

    #[tokio::test]
    async fn message_delivery_keeps_bare_approval_responses_unprefixed() {
        let state = AppState::new();
        insert_test_agent(&state, "source-1", "PlannerOne", "Planner").await;
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Action Needed".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "y",
            None,
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"y".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
    }

    #[tokio::test]
    async fn message_delivery_prefixes_bare_approval_response_when_target_not_action_needed() {
        let state = AppState::new();
        insert_test_agent(&state, "source-1", "PlannerOne", "Planner").await;
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "yes",
            None,
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            rx.recv().await.unwrap(),
            b"From PlannerOne: yes".to_vec()
        );
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
    }

    #[tokio::test]
    async fn message_delivery_reports_missing_target_as_not_found() {
        let state = AppState::new();

        let error = deliver_message_to_target(None, &state, "ghost", "hello", None, None)
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

        let error = deliver_message_to_target(None, &state, "agent-1", "hello", None, None)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "request_failed");
        assert!(error.to_string().contains("agent-1: no input channel"));
        assert_eq!(
            error.details().unwrap()["delivery"][0]["runtime_state"],
            "restored_without_sender"
        );
        assert_eq!(
            error.details().unwrap()["delivery"][0]["error"]["code"],
            "no_input_channel"
        );
    }

    #[tokio::test]
    async fn message_delivery_reports_partial_failures_after_successful_delivery() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        insert_test_agent(&state, "agent-2", "CoderTwo", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let error = deliver_message_to_target(None, &state, "class:Coder", "hello", None, None)
            .await
            .unwrap_err();

        assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
        assert_eq!(error.code(), "request_failed");
        assert!(error
            .to_string()
            .contains("message delivery failed for 1 of 2 matched agents"));
        assert!(error.to_string().contains("agent-2: no input channel"));
        let details = error.details().unwrap()["delivery"]
            .as_array()
            .expect("delivery details");
        let failed = details
            .iter()
            .find(|detail| detail["uuid"] == "agent-2")
            .expect("failed agent detail");
        assert_eq!(failed["delivery_state"], "failed");
    }

    #[tokio::test]
    async fn delivery_attempt_records_watch_event() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        deliver_message_to_target(None, &state, "CoderOne", "hello", None, None)
            .await
            .unwrap();

        assert!(rx.recv().await.is_some());
        let agents = state.agents.lock().await;
        let agent = agents.get("agent-1").unwrap();
        let snapshot = agent
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, Some(4096))
            .unwrap();
        assert!(snapshot.events.iter().any(|event| event.kind == "delivery"));
    }

    #[test]
    fn watch_target_rejects_multi_target_selectors() {
        assert_eq!(
            validate_watch_target("all").unwrap_err().code(),
            "not_supported"
        );
        assert_eq!(
            validate_watch_target("class:Coder").unwrap_err().code(),
            "not_supported"
        );
    }

    #[test]
    fn follow_flag_is_reserved_not_supported() {
        let error = validate_watch_follow(false).err();
        assert!(error.is_none());

        let error = validate_watch_follow(true).unwrap_err();
        assert_eq!(error.code(), "not_supported");
    }

    #[tokio::test]
    async fn blocking_watch_wakes_when_output_arrives() {
        let state = std::sync::Arc::new(std::sync::Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            1024,
        )));
        let cursor = state.lock().unwrap().latest_cursor();
        let writer = state.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            writer.lock().unwrap().push_output(b"WARDIAN_OK");
        });

        let snapshot = wait_for_watch_condition(
            state,
            Some(cursor),
            WatchCondition::OutputContains("WARDIAN_OK".to_string()),
            std::time::Duration::from_secs(1),
            Some(1024),
        )
        .await
        .unwrap();

        assert!(snapshot.output.text.contains("WARDIAN_OK"));
    }

    #[tokio::test]
    async fn blocking_watch_reports_gap_when_cursor_expires_while_waiting() {
        let state = std::sync::Arc::new(std::sync::Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            2,
            1024,
        )));
        let cursor = state.lock().unwrap().latest_cursor();
        let writer = state.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let mut guard = writer.lock().unwrap();
            guard.push_event("status", serde_json::json!({"status":"processing"}));
            guard.push_event("status", serde_json::json!({"status":"idle"}));
            guard.push_event("status", serde_json::json!({"status":"processing"}));
        });

        let error = wait_for_watch_condition(
            state,
            Some(cursor),
            WatchCondition::OutputContains("never".to_string()),
            std::time::Duration::from_secs(1),
            Some(1024),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "gap_detected");
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
