use crate::state::AppState;
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{
    AgentListResponse, AgentResponse, AgentWatchResponse, AgentWorktreeListResponse,
    AgentWorktreeMutationResponse, AgentWorktreeSummary, AskResponse, ControlRequest,
    DeliveryDetail, DeliveryErrorDetail, MessageInputMode, MessageOrigin, OkResponse,
    ReplyResponse, ReplyStatus, SendMessageResponse, StructuredReply, WatchAgentSnapshot,
    WatchDeliverySnapshot, WorkflowListResponse, WorkflowResponse, WorkflowSummary,
};
use wardian_core::identity::{normalize_status, AgentIdentity, StatusSource};

const STRUCTURED_ASK_INLINE_MESSAGE_MAX_BYTES: usize = 4096;
const STRUCTURED_ASK_REQUESTS_DIR: &str = "requests";

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

        ControlRequest::AgentWorktreeList => {
            let state = app.state::<AppState>();
            let worktrees = list_agent_worktree_summaries(state).await?;
            ok_json(&AgentWorktreeListResponse::new(worktrees))
        }

        ControlRequest::AgentWorktreeEnable { target, name } => {
            handle_agent_worktree_enable(app, &target, name).await
        }

        ControlRequest::AgentWorktreeJoin { target, worktree } => {
            handle_agent_worktree_join(app, &target, &worktree).await
        }

        ControlRequest::AgentWorktreeDisable { target } => {
            handle_agent_worktree_disable(app, &target).await
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
            input_mode,
            origin,
        } => {
            let state = app.state::<AppState>();
            let delivery = deliver_message_to_target(
                Some(app),
                &state,
                &target,
                &message,
                thread.as_deref(),
                input_mode,
                origin.as_ref(),
            )
            .await?;
            ok_json(&SendMessageResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                ok: true,
                delivery,
            })
        }

        ControlRequest::Ask {
            target,
            message,
            thread,
            tail_bytes,
            timeout_ms,
            origin,
        } => {
            handle_structured_ask(
                app,
                &target,
                &message,
                thread.as_deref(),
                tail_bytes,
                Duration::from_millis(timeout_ms.unwrap_or(30_000)),
                origin.as_ref(),
            )
            .await
        }

        ControlRequest::SubmitReply {
            request_id,
            status,
            body,
            origin,
        } => {
            let state = app.state::<AppState>();
            let reply =
                submit_structured_reply(&state, &request_id, status, &body, origin.as_ref())
                    .await?;
            ok_json(&ReplyResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                ok: true,
                request_id,
                reply,
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
            output_echo_guard,
        } => {
            handle_agent_watch(
                app,
                &target,
                AgentWatchControlOptions {
                    since,
                    until,
                    tail_bytes,
                    follow,
                    timeout_ms,
                    output_echo_guard,
                },
            )
            .await
        }
    }
}

fn build_spawn_agent_request(
    provider: String,
    class: String,
    name: Option<String>,
    workspace: Option<String>,
) -> crate::commands::agent::SpawnAgentRequest {
    let mut config_override = wardian_core::models::AgentConfig {
        provider,
        ..Default::default()
    };
    config_override.reset_provider_config_for_provider();
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

async fn list_agent_worktree_summaries(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<AgentWorktreeSummary>, ControlError> {
    crate::commands::agent::list_agent_worktrees(state)
        .await
        .map(|worktrees| worktrees.into_iter().map(core_worktree_summary).collect())
        .map_err(ControlError::request_failed)
}

fn core_worktree_summary(
    summary: crate::commands::agent::AgentWorktreeSummary,
) -> AgentWorktreeSummary {
    AgentWorktreeSummary {
        id: summary.id,
        name: summary.name,
        source_folder: summary.source_folder,
        worktree_folder: summary.worktree_folder,
        member_agent_ids: summary.member_agent_ids,
    }
}

fn worktree_for_member(
    worktrees: &[AgentWorktreeSummary],
    session_id: &str,
) -> Option<AgentWorktreeSummary> {
    worktrees
        .iter()
        .find(|worktree| {
            worktree
                .member_agent_ids
                .iter()
                .any(|member_id| member_id == session_id)
        })
        .cloned()
}

fn worktree_by_folder(
    worktrees: &[AgentWorktreeSummary],
    folder: &str,
) -> Option<AgentWorktreeSummary> {
    let normalized = folder.trim().replace('\\', "/");
    worktrees
        .iter()
        .find(|worktree| worktree.worktree_folder == normalized || worktree.id == normalized)
        .cloned()
}

async fn handle_agent_worktree_enable(
    app: &AppHandle,
    target: &str,
    name: Option<String>,
) -> Result<String, ControlError> {
    let uuid = resolve_target_uuid(app, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let previous_workspace = agent_workspace(app, &uuid).await;
    let branch_name = agent_worktree_branch_name(app, &uuid, name.as_deref()).await?;

    let state = app.state::<AppState>();
    crate::commands::agent::enable_agent_worktree(uuid.clone(), name, state, app.clone())
        .await
        .map_err(ControlError::request_failed)?;
    clear_agent_after_worktree_move(app, &uuid).await?;

    let worktrees = list_agent_worktree_summaries(app.state::<AppState>()).await?;
    let worktree = worktree_for_member(&worktrees, &uuid);
    let agent = live_agent_identity(app, &uuid).await?;
    let response = AgentWorktreeMutationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        action: "enable".to_string(),
        previous_workspace,
        current_workspace: agent.workspace.clone(),
        agent,
        worktree,
        previous_worktree: None,
        branch_name: Some(branch_name),
        cleared_session: true,
    };
    ok_json(&response)
}

async fn handle_agent_worktree_join(
    app: &AppHandle,
    target: &str,
    worktree: &str,
) -> Result<String, ControlError> {
    let uuid = resolve_target_uuid(app, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let previous_workspace = agent_workspace(app, &uuid).await;
    let state = app.state::<AppState>();
    let before = list_agent_worktree_summaries(app.state::<AppState>()).await?;
    let target_worktree = worktree_by_folder(&before, worktree).ok_or_else(|| {
        ControlError::coded(
            "not_managed_worktree",
            format!("worktree is not managed by Wardian: {worktree}"),
        )
    })?;

    crate::commands::agent::assign_agent_worktree(
        uuid.clone(),
        target_worktree.worktree_folder.clone(),
        state,
        app.clone(),
    )
    .await
    .map_err(ControlError::request_failed)?;
    clear_agent_after_worktree_move(app, &uuid).await?;

    let worktrees = list_agent_worktree_summaries(app.state::<AppState>()).await?;
    let agent = live_agent_identity(app, &uuid).await?;
    let response = AgentWorktreeMutationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        action: "join".to_string(),
        previous_workspace,
        current_workspace: agent.workspace.clone(),
        agent,
        worktree: worktree_for_member(&worktrees, &uuid).or(Some(target_worktree)),
        previous_worktree: None,
        branch_name: None,
        cleared_session: true,
    };
    ok_json(&response)
}

async fn handle_agent_worktree_disable(
    app: &AppHandle,
    target: &str,
) -> Result<String, ControlError> {
    let uuid = resolve_target_uuid(app, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let previous_workspace = agent_workspace(app, &uuid).await;
    let before = list_agent_worktree_summaries(app.state::<AppState>()).await?;
    let previous_worktree = worktree_for_member(&before, &uuid);

    let state = app.state::<AppState>();
    crate::commands::agent::disable_agent_worktree(uuid.clone(), state, app.clone())
        .await
        .map_err(ControlError::request_failed)?;
    clear_agent_after_worktree_move(app, &uuid).await?;

    let agent = live_agent_identity(app, &uuid).await?;
    let response = AgentWorktreeMutationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        action: "disable".to_string(),
        previous_workspace,
        current_workspace: agent.workspace.clone(),
        agent,
        worktree: None,
        previous_worktree,
        branch_name: None,
        cleared_session: true,
    };
    ok_json(&response)
}

async fn clear_agent_after_worktree_move(
    app: &AppHandle,
    session_id: &str,
) -> Result<(), ControlError> {
    crate::commands::agent::clear_agent_session(
        session_id.to_string(),
        app.state::<AppState>(),
        app.clone(),
    )
    .await
    .map_err(ControlError::request_failed)
}

async fn agent_worktree_branch_name(
    app: &AppHandle,
    session_id: &str,
    requested_name: Option<&str>,
) -> Result<String, ControlError> {
    let state = app.state::<AppState>();
    let agents = state.agents.lock().await;
    let agent = agents
        .get(session_id)
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {session_id}")))?;
    let config = agent
        .config
        .lock()
        .map_err(|_| ControlError::request_failed("agent config lock poisoned"))?;
    let source = requested_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&config.session_name);
    Ok(crate::commands::agent::resolve_agent_worktree_branch_name(
        source,
    ))
}

async fn agent_workspace(app: &AppHandle, session_id: &str) -> Option<String> {
    let state = app.state::<AppState>();
    let agents = state.agents.lock().await;
    let agent = agents.get(session_id)?;
    let config = agent.config.lock().ok()?;
    (!config.folder.trim().is_empty()).then(|| config.folder.clone())
}

async fn live_agent_identity(
    app: &AppHandle,
    session_id: &str,
) -> Result<AgentIdentity, ControlError> {
    live_agent_snapshots(app)
        .await
        .into_iter()
        .find(|agent| agent.uuid == session_id)
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {session_id}")))
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
    input_mode: MessageInputMode,
    origin: Option<&MessageOrigin>,
) -> Result<Vec<DeliveryDetail>, ControlError> {
    validate_send_message_options(target, thread, input_mode)?;
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
                let outbound_message = message_with_origin(
                    state,
                    message,
                    input_mode,
                    origin,
                    info.status == "action_required",
                )
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
                            input_mode,
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
                            input_mode,
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
                    input_mode,
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
    input_mode: MessageInputMode,
    origin: Option<&MessageOrigin>,
    allow_bare_approval_response: bool,
) -> String {
    if input_mode == MessageInputMode::Command {
        return message.to_string();
    }

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
    strip_ansi_controls(output)
        .replace('\r', "\n")
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .is_some_and(|line| line.starts_with('›'))
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

async fn handle_structured_ask(
    app: &AppHandle,
    target: &str,
    message: &str,
    thread: Option<&str>,
    tail_bytes: Option<usize>,
    timeout: Duration,
    origin: Option<&MessageOrigin>,
) -> Result<String, ControlError> {
    validate_send_message_thread(thread)?;
    validate_watch_target(target)?;
    let state = app.state::<AppState>();
    let target_uuid = resolve_target_uuid_in_state(&state, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let watch_state = agent_watch_state(&state, &target_uuid).await?;
    let initial_cursor = watch_state
        .lock()
        .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?
        .latest_cursor();
    let request_id = new_ask_request_id();
    let wardian_home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| ControlError::request_failed("could not resolve Wardian home"))?;
    let structured_delivery =
        build_structured_ask_delivery_message(&wardian_home, &target_uuid, message, &request_id)?;
    create_pending_ask_request_with_id(
        &state,
        &target_uuid,
        request_id.clone(),
        structured_delivery.body_file.as_deref(),
    )
    .await?;
    let delivery = deliver_message_to_target(
        Some(app),
        &state,
        target,
        &structured_delivery.prompt,
        thread,
        MessageInputMode::Message,
        origin,
    )
    .await?;
    let reply = wait_for_structured_reply(&state, &request_id, timeout).await?;
    let snapshot = watch_state
        .lock()
        .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?
        .snapshot_since(Some(&initial_cursor), tail_bytes)
        .map_err(control_error_from_watch_state)?;
    let agent = watch_agent_snapshot(&state, &target_uuid).await?;

    let delivery_snapshot = delivery_snapshot_from_events(&snapshot.events);
    ok_json(&AskResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        request_id,
        target: target.to_string(),
        delivery,
        reply,
        watch: AgentWatchResponse {
            schema: wardian_core::control::CONTROL_SCHEMA,
            agent,
            cursor: snapshot.cursor,
            events: snapshot.events,
            output: snapshot.output,
            delivery: delivery_snapshot,
        },
    })
}

fn message_with_structured_reply_instruction(message: &str, request_id: &str) -> String {
    format!(
        "{message}\n\nWardian request id: {request_id}\nRespond to this request with:\nwardian reply {request_id} --status done --stdin\nUse --status blocked or --status failed if you cannot complete it. Put the reply body on stdin."
    )
}

#[derive(Debug)]
struct StructuredAskDeliveryMessage {
    prompt: String,
    body_file: Option<PathBuf>,
}

fn build_structured_ask_delivery_message(
    wardian_home: &Path,
    target_session_id: &str,
    message: &str,
    request_id: &str,
) -> Result<StructuredAskDeliveryMessage, ControlError> {
    if message.len() <= STRUCTURED_ASK_INLINE_MESSAGE_MAX_BYTES {
        return Ok(StructuredAskDeliveryMessage {
            prompt: message_with_structured_reply_instruction(message, request_id),
            body_file: None,
        });
    }

    let body_file = wardian_home
        .join("agents")
        .join(target_session_id)
        .join("habitat")
        .join(STRUCTURED_ASK_REQUESTS_DIR)
        .join(format!("{request_id}.md"));
    if let Some(parent) = body_file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            ControlError::request_failed(format!("failed to create ask request directory: {error}"))
        })?;
    }
    std::fs::write(&body_file, message).map_err(|error| {
        ControlError::request_failed(format!("failed to write ask request body: {error}"))
    })?;

    Ok(StructuredAskDeliveryMessage {
        prompt: message_with_structured_reply_instruction(
            &format!(
                "Wardian structured request {request_id} is too large to paste safely.\nRead the full request body from:\n{}",
                body_file.display()
            ),
            request_id,
        ),
        body_file: Some(body_file),
    })
}

fn new_ask_request_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    format!("ask_{:016x}", nanos ^ counter)
}

#[cfg(test)]
async fn create_pending_ask_request(
    state: &AppState,
    target_session_id: &str,
) -> Result<String, ControlError> {
    let request_id = new_ask_request_id();
    create_pending_ask_request_with_id(state, target_session_id, request_id.clone(), None).await?;
    Ok(request_id)
}

async fn create_pending_ask_request_with_id(
    state: &AppState,
    target_session_id: &str,
    request_id: String,
    body_file: Option<&Path>,
) -> Result<(), ControlError> {
    if !state.agents.lock().await.contains_key(target_session_id) {
        return Err(ControlError::not_found(format!(
            "agent not found: {target_session_id}"
        )));
    }

    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    state.ask_requests.lock().await.insert(
        request_id.clone(),
        crate::state::app_state::AskRequestRecord {
            request_id: request_id.clone(),
            target_session_id: target_session_id.to_string(),
            created_at: created_at.clone(),
            reply: None,
        },
    );
    let mut payload = serde_json::json!({
        "request_id": request_id,
        "target_session_id": target_session_id,
        "status": "pending",
        "created_at": created_at,
    });
    if let Some(body_file) = body_file {
        if let Some(payload) = payload.as_object_mut() {
            payload.insert(
                "body_file".to_string(),
                serde_json::Value::String(body_file.display().to_string()),
            );
        }
    }
    push_watch_event_for_agent(state, target_session_id, "request", payload).await?;
    Ok(())
}

async fn submit_structured_reply(
    state: &AppState,
    request_id: &str,
    status: ReplyStatus,
    body: &str,
    origin: Option<&MessageOrigin>,
) -> Result<StructuredReply, ControlError> {
    let source_session_id =
        origin.map(|MessageOrigin::WardianAgent { session_id }| session_id.clone());

    let reply = {
        let mut requests = state.ask_requests.lock().await;
        let request = requests.get_mut(request_id).ok_or_else(|| {
            ControlError::not_found(format!("ask request not found: {request_id}"))
        })?;
        if let Some(source) = &source_session_id {
            if source != &request.target_session_id {
                return Err(ControlError::coded(
                    "unauthorized",
                    "reply origin does not match ask target",
                ));
            }
        }
        if request.reply.is_some() {
            return Err(ControlError::coded(
                "duplicate_reply",
                "ask request already has a terminal reply",
            ));
        }

        let replied_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let reply = StructuredReply {
            request_id: request.request_id.clone(),
            status,
            body: body.to_string(),
            target_session_id: request.target_session_id.clone(),
            source_session_id,
            replied_at,
        };
        request.reply = Some(reply.clone());
        reply
    };

    push_watch_event_for_agent(
        state,
        &reply.target_session_id,
        "reply",
        serde_json::json!({
            "request_id": reply.request_id,
            "status": reply.status,
            "target_session_id": reply.target_session_id,
            "source_session_id": reply.source_session_id,
            "replied_at": reply.replied_at,
        }),
    )
    .await?;
    Ok(reply)
}

async fn wait_for_structured_reply(
    state: &AppState,
    request_id: &str,
    timeout: Duration,
) -> Result<StructuredReply, ControlError> {
    let started = std::time::Instant::now();
    loop {
        let reply = {
            let requests = state.ask_requests.lock().await;
            let request = requests.get(request_id).ok_or_else(|| {
                ControlError::not_found(format!("ask request not found: {request_id}"))
            })?;
            request.reply.clone()
        };
        if let Some(reply) = reply {
            return Ok(reply);
        }
        if started.elapsed() >= timeout {
            return Err(
                ControlError::watch_timeout("structured reply timed out").with_details(
                    serde_json::json!({
                        "request_id": request_id,
                        "until": "reply",
                    }),
                ),
            );
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        tokio::time::sleep(remaining.min(Duration::from_millis(25))).await;
    }
}

async fn push_watch_event_for_agent(
    state: &AppState,
    session_id: &str,
    kind: &str,
    payload: serde_json::Value,
) -> Result<(), ControlError> {
    let agents = state.agents.lock().await;
    let agent = agents
        .get(session_id)
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {session_id}")))?;
    agent
        .watch_state
        .lock()
        .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?
        .push_event(kind, payload);
    Ok(())
}

async fn handle_agent_watch(
    app: &AppHandle,
    target: &str,
    options: AgentWatchControlOptions,
) -> Result<String, ControlError> {
    validate_watch_follow(options.follow)?;
    validate_watch_target(target)?;
    let condition = options
        .until
        .as_deref()
        .map(parse_watch_condition)
        .transpose()?;
    let state = app.state::<AppState>();
    let uuid = resolve_target_uuid_in_state(&state, target)
        .await
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
    let watch_state = agent_watch_state(&state, &uuid).await?;
    let snapshot = if let Some(condition) = condition {
        wait_for_watch_condition(
            watch_state,
            options.since,
            condition,
            Duration::from_millis(options.timeout_ms.unwrap_or(30_000)),
            options.tail_bytes,
            options.output_echo_guard,
        )
        .await?
    } else {
        watch_state
            .lock()
            .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?
            .snapshot_since(options.since.as_deref(), options.tail_bytes)
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

struct AgentWatchControlOptions {
    since: Option<String>,
    until: Option<String>,
    tail_bytes: Option<usize>,
    follow: bool,
    timeout_ms: Option<u64>,
    output_echo_guard: Option<String>,
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
    output_echo_guard: Option<String>,
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
            Ok(snapshot)
                if watch_condition_matches(&condition, &snapshot, output_echo_guard.as_deref()) =>
            {
                return Ok(snapshot)
            }
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
    output_echo_guard: Option<&str>,
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
        WatchCondition::OutputContains(token) => {
            snapshot.output.text.contains(token)
                && !output_match_is_prompt_echo_only(
                    token,
                    &snapshot.output.text,
                    output_echo_guard,
                )
        }
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

fn output_match_is_prompt_echo_only(
    token: &str,
    output_text: &str,
    submitted_message: Option<&str>,
) -> bool {
    let Some(submitted_message) = submitted_message else {
        return false;
    };
    if token.is_empty() || !submitted_message.contains(token) {
        return false;
    }
    let output_lines = normalized_echo_lines(output_text);
    if output_lines.is_empty() {
        return false;
    }
    let submitted_joined = normalized_echo_lines(submitted_message).join(" ");
    if submitted_joined.is_empty() {
        return false;
    }

    let mut saw_token = false;
    for line in output_lines.iter().filter(|line| line.contains(token)) {
        saw_token = true;
        if !normalized_line_is_submitted_prompt_echo(line, &submitted_joined, token) {
            return false;
        }
    }
    saw_token
}

fn normalized_line_is_submitted_prompt_echo(
    line: &str,
    submitted_joined: &str,
    token: &str,
) -> bool {
    prompt_echo_line_candidates(line).iter().any(|candidate| {
        if submitted_joined.contains(candidate.as_str())
            && !(candidate == token && submitted_joined != token)
        {
            return true;
        }
        candidate_contains_submitted_prompt_fragment(candidate, submitted_joined, token)
    })
}

fn prompt_echo_line_candidates(line: &str) -> Vec<String> {
    let mut candidates = vec![line.to_string(), strip_origin_prefix(line).to_string()];
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(content) = json.get("content").and_then(|value| value.as_str()) {
            let normalized_content = content.split_whitespace().collect::<Vec<_>>().join(" ");
            candidates.push(normalized_content.clone());
            candidates.push(strip_origin_prefix(&normalized_content).to_string());
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn candidate_contains_submitted_prompt_fragment(
    candidate: &str,
    submitted_joined: &str,
    token: &str,
) -> bool {
    if !candidate.contains(token) {
        return false;
    }

    let candidate_words = normalized_prompt_words(candidate);
    let submitted_words = normalized_prompt_words(submitted_joined);
    if candidate_words.is_empty() || submitted_words.len() < 2 {
        return false;
    }

    let min_phrase_words = submitted_words.len().min(3);
    if candidate_words.len() < min_phrase_words {
        return false;
    }

    let max_phrase_words = candidate_words.len().min(submitted_words.len());
    (min_phrase_words..=max_phrase_words)
        .rev()
        .any(|phrase_words| {
            candidate_words
                .windows(phrase_words)
                .any(|candidate_window| {
                    candidate_window.iter().any(|word| word.contains(token))
                        && submitted_words
                            .windows(phrase_words)
                            .any(|submitted_window| submitted_window == candidate_window)
                })
        })
}

fn normalized_prompt_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|ch: char| ch.is_ascii_punctuation() && ch != '_' && ch != '-')
        })
        .filter(|word| !word.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn strip_origin_prefix(line: &str) -> &str {
    line.strip_prefix("From ")
        .and_then(|without_from| without_from.split_once(": ").map(|(_, rest)| rest))
        .unwrap_or(line)
}

fn normalized_echo_lines(text: &str) -> Vec<String> {
    strip_ansi_controls(text)
        .replace('\r', "\n")
        .lines()
        .filter_map(normalized_echo_line)
        .collect()
}

fn normalized_echo_line(line: &str) -> Option<String> {
    let trimmed = line.trim().trim_start_matches(is_prompt_prefix_char).trim();
    let normalized = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

fn is_prompt_prefix_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '›' | '>' | '$' | '#' | ':' | '|' | '│' | '┃' | '»' | '•' | '·' | '-' | '*'
        )
}

fn strip_ansi_controls(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            stripped.push(ch);
        }
    }
    stripped
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

fn validate_send_message_options(
    target: &str,
    thread: Option<&str>,
    input_mode: MessageInputMode,
) -> Result<(), ControlError> {
    validate_send_message_thread(thread)?;

    if input_mode == MessageInputMode::Command && (target == "all" || target.starts_with("class:"))
    {
        return Err(ControlError::not_supported(
            "--as-command requires a single agent name or uuid",
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
    input_mode: MessageInputMode,
) -> DeliveryDetail {
    DeliveryDetail {
        uuid: info.uuid,
        name: info.name,
        provider: info.provider,
        runtime_state: runtime_state.to_string(),
        delivery_state: "failed".to_string(),
        input_mode,
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

    #[test]
    fn codex_ready_prompt_ignores_stale_prompt_marker_when_latest_screen_is_busy() {
        assert!(!codex_output_has_ready_prompt(
            "\r\n› Previous prompt\r\nProcessing request\r\nWorking...\r\n"
        ));
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

        deliver_message_to_target(
            None,
            &state,
            "OpenCodeOne",
            "hello",
            None,
            MessageInputMode::Message,
            None,
        )
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
        assert!(matches!(
            req.config_override
                .as_ref()
                .map(|config| &config.provider_config),
            Some(wardian_core::models::ProviderConfig::Codex(_))
        ));
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

    #[test]
    fn worktree_by_folder_matches_normalized_folder_or_id() {
        let worktrees = vec![AgentWorktreeSummary {
            id: "C:/repo/worktrees/review".to_string(),
            name: "review".to_string(),
            source_folder: "C:/repo".to_string(),
            worktree_folder: "C:/repo/worktrees/review".to_string(),
            member_agent_ids: vec!["agent-1".to_string()],
        }];

        let matched = worktree_by_folder(&worktrees, "C:\\repo\\worktrees\\review").unwrap();

        assert_eq!(matched.id, "C:/repo/worktrees/review");
    }

    #[test]
    fn worktree_for_member_returns_member_summary() {
        let worktrees = vec![AgentWorktreeSummary {
            id: "C:/repo/worktrees/review".to_string(),
            name: "review".to_string(),
            source_folder: "C:/repo".to_string(),
            worktree_folder: "C:/repo/worktrees/review".to_string(),
            member_agent_ids: vec!["agent-1".to_string(), "agent-2".to_string()],
        }];

        assert_eq!(
            worktree_for_member(&worktrees, "agent-2")
                .unwrap()
                .name
                .as_str(),
            "review"
        );
        assert!(worktree_for_member(&worktrees, "missing").is_none());
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

        deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "hello",
            None,
            MessageInputMode::Message,
            None,
        )
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
            MessageInputMode::Message,
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
    async fn command_delivery_keeps_origin_unattributed_and_records_input_mode() {
        let state = AppState::new();
        insert_test_agent(&state, "source-1", "PlannerOne", "Planner").await;
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let delivery = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "/goal test",
            None,
            MessageInputMode::Command,
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"/goal test".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
        assert_eq!(delivery[0].input_mode, MessageInputMode::Command);
    }

    #[test]
    fn command_delivery_rejects_multi_target_selectors() {
        let all =
            validate_send_message_options("all", None, MessageInputMode::Command).unwrap_err();
        let class = validate_send_message_options("class:Coder", None, MessageInputMode::Command)
            .unwrap_err();

        assert_eq!(all.code(), "not_supported");
        assert_eq!(class.code(), "not_supported");
        assert!(all
            .to_string()
            .contains("--as-command requires a single agent name or uuid"));
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
            MessageInputMode::Message,
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
            MessageInputMode::Message,
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"From PlannerOne: yes".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
    }

    #[tokio::test]
    async fn message_delivery_reports_missing_target_as_not_found() {
        let state = AppState::new();

        let error = deliver_message_to_target(
            None,
            &state,
            "ghost",
            "hello",
            None,
            MessageInputMode::Message,
            None,
        )
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

        let error = deliver_message_to_target(
            None,
            &state,
            "agent-1",
            "hello",
            None,
            MessageInputMode::Message,
            None,
        )
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

        let error = deliver_message_to_target(
            None,
            &state,
            "class:Coder",
            "hello",
            None,
            MessageInputMode::Message,
            None,
        )
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

        deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "hello",
            None,
            MessageInputMode::Message,
            None,
        )
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
    fn generated_ask_request_id_has_stable_shape() {
        let request_id = new_ask_request_id();
        let Some(suffix) = request_id.strip_prefix("ask_") else {
            panic!("request id should use ask_ prefix: {request_id}");
        };
        assert_eq!(suffix.len(), 16);
        assert!(suffix.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn long_structured_ask_materializes_body_file_and_sends_short_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let request_id = "ask_testrequest01";
        let message = "investigate this line\n".repeat(STRUCTURED_ASK_INLINE_MESSAGE_MAX_BYTES);

        let delivery =
            build_structured_ask_delivery_message(temp.path(), "agent-1", &message, request_id)
                .unwrap();

        let body_file = delivery
            .body_file
            .expect("long ask body should be materialized");
        assert_eq!(
            body_file,
            temp.path()
                .join("agents")
                .join("agent-1")
                .join("habitat")
                .join("requests")
                .join(format!("{request_id}.md"))
        );
        assert_eq!(std::fs::read_to_string(&body_file).unwrap(), message);
        assert!(delivery.prompt.contains(request_id));
        assert!(delivery.prompt.contains("Read the full request body from:"));
        assert!(delivery
            .prompt
            .contains(&format!("wardian reply {request_id} --status done --stdin")));
        assert!(
            !delivery
                .prompt
                .contains("investigate this line\ninvestigate this line"),
            "large body should not be pasted into the terminal prompt"
        );
    }

    #[tokio::test]
    async fn ask_request_lifecycle_accepts_matching_reply_and_emits_watch_event() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let request_id = create_pending_ask_request(&state, "agent-1").await.unwrap();

        let reply = submit_structured_reply(
            &state,
            &request_id,
            wardian_core::control::ReplyStatus::Done,
            "finished",
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "agent-1".to_string(),
            }),
        )
        .await
        .unwrap();

        assert_eq!(reply.request_id, request_id);
        assert_eq!(reply.status, wardian_core::control::ReplyStatus::Done);
        assert_eq!(reply.body, "finished");
        assert_eq!(reply.source_session_id.as_deref(), Some("agent-1"));

        let agents = state.agents.lock().await;
        let snapshot = agents
            .get("agent-1")
            .unwrap()
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, Some(4096))
            .unwrap();
        assert!(snapshot
            .events
            .iter()
            .any(|event| { event.kind == "request" && event.payload["request_id"] == request_id }));
        assert!(snapshot.events.iter().any(|event| {
            event.kind == "reply"
                && event.payload["request_id"] == request_id
                && event.payload["status"] == "done"
        }));
    }

    #[tokio::test]
    async fn ask_request_event_records_materialized_body_file() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let body_file = PathBuf::from("agents/agent-1/habitat/requests/ask_test.md");

        create_pending_ask_request_with_id(
            &state,
            "agent-1",
            "ask_testrequest02".to_string(),
            Some(&body_file),
        )
        .await
        .unwrap();

        let agents = state.agents.lock().await;
        let snapshot = agents
            .get("agent-1")
            .unwrap()
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, Some(4096))
            .unwrap();
        assert!(snapshot.events.iter().any(|event| {
            event.kind == "request"
                && event.payload["request_id"] == "ask_testrequest02"
                && event.payload["body_file"] == body_file.display().to_string()
        }));
    }

    #[tokio::test]
    async fn ask_reply_rejects_unknown_duplicate_and_foreign_request() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        insert_test_agent(&state, "agent-2", "CoderTwo", "Coder").await;

        let unknown = submit_structured_reply(
            &state,
            "ask_deadbeefdeadbeef",
            wardian_core::control::ReplyStatus::Done,
            "finished",
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(unknown.code(), "not_found");

        let request_id = create_pending_ask_request(&state, "agent-1").await.unwrap();
        let foreign = submit_structured_reply(
            &state,
            &request_id,
            wardian_core::control::ReplyStatus::Done,
            "finished",
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "agent-2".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(foreign.code(), "unauthorized");

        submit_structured_reply(
            &state,
            &request_id,
            wardian_core::control::ReplyStatus::Blocked,
            "blocked on review",
            None,
        )
        .await
        .unwrap();
        let duplicate = submit_structured_reply(
            &state,
            &request_id,
            wardian_core::control::ReplyStatus::Done,
            "finished",
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(duplicate.code(), "duplicate_reply");
    }

    #[tokio::test]
    async fn wait_for_structured_reply_times_out_without_terminal_status() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let request_id = create_pending_ask_request(&state, "agent-1").await.unwrap();

        let error =
            wait_for_structured_reply(&state, &request_id, std::time::Duration::from_millis(10))
                .await
                .unwrap_err();

        assert_eq!(error.code(), "watch_timeout");
    }

    #[tokio::test]
    async fn wait_for_structured_reply_returns_blocked_and_failed_statuses() {
        for status in [
            wardian_core::control::ReplyStatus::Blocked,
            wardian_core::control::ReplyStatus::Failed,
        ] {
            let state = AppState::new();
            insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
            let request_id = create_pending_ask_request(&state, "agent-1").await.unwrap();
            submit_structured_reply(&state, &request_id, status.clone(), "cannot continue", None)
                .await
                .unwrap();

            let reply =
                wait_for_structured_reply(&state, &request_id, std::time::Duration::from_secs(1))
                    .await
                    .unwrap();

            assert_eq!(reply.status, status);
            assert_eq!(reply.body, "cannot continue");
        }
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
            None,
        )
        .await
        .unwrap();

        assert!(snapshot.output.text.contains("WARDIAN_OK"));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_ignores_submitted_prompt_echo() {
        let snapshot = crate::state::agent_watch::WatchSnapshot {
            cursor: "agent-1:0000000000000001".to_string(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: "agent-1:0000000000000001".to_string(),
                text: "\u{1b}[1m›\u{1b}[22m From Wardian-Arch: Say AUTO_TEST_2_DONE when finished\r\n  gpt-5.5 high · D:\\Development\\Wardian".to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
        };

        assert!(!watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("Say AUTO_TEST_2_DONE when finished"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_matches_provider_response_after_echo() {
        let snapshot = crate::state::agent_watch::WatchSnapshot {
            cursor: "agent-1:0000000000000002".to_string(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: "agent-1:0000000000000002".to_string(),
                text: "\u{1b}[1m›\u{1b}[22m Say AUTO_TEST_2_DONE when finished\r\nActual response: AUTO_TEST_2_DONE".to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
        };

        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("Say AUTO_TEST_2_DONE when finished"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_ignores_codex_repaint_prompt_fragment() {
        let snapshot = crate::state::agent_watch::WatchSnapshot {
            cursor: "agent-1:0000000000000003".to_string(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: "agent-1:0000000000000003".to_string(),
                text: "\u{1b}[2J\u{1b}[H\u{1b}[1m›\u{1b}[22m From Wardian-Arch: Capture the README demo GIF\r\n  and end exactly with DEMO_GIF_DONE  gpt-5.5 high · D:\\Development\\Wardian · 75% context left".to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
        };

        assert!(!watch_condition_matches(
            &WatchCondition::OutputContains("DEMO_GIF_DONE".to_string()),
            &snapshot,
            Some("Capture the README demo GIF and end exactly with DEMO_GIF_DONE"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_matches_response_after_codex_repaint_echo() {
        let snapshot = crate::state::agent_watch::WatchSnapshot {
            cursor: "agent-1:0000000000000004".to_string(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: "agent-1:0000000000000004".to_string(),
                text: "\u{1b}[2J\u{1b}[H\u{1b}[1m›\u{1b}[22m From Wardian-Arch: Capture the README demo GIF\r\n  and end exactly with DEMO_GIF_DONE  gpt-5.5 high · D:\\Development\\Wardian · 75% context left\r\nFinal response: DEMO_GIF_DONE".to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
        };

        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("DEMO_GIF_DONE".to_string()),
            &snapshot,
            Some("Capture the README demo GIF and end exactly with DEMO_GIF_DONE"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_matches_exact_marker_response_after_echo() {
        let snapshot = crate::state::agent_watch::WatchSnapshot {
            cursor: "agent-1:0000000000000005".to_string(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: "agent-1:0000000000000005".to_string(),
                text:
                    "\u{1b}[1m›\u{1b}[22m Say AUTO_TEST_2_DONE when finished\r\n  AUTO_TEST_2_DONE"
                        .to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
        };

        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("Say AUTO_TEST_2_DONE when finished"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_ignores_origin_prefixed_json_echo() {
        let snapshot = crate::state::agent_watch::WatchSnapshot {
            cursor: "agent-1:0000000000000006".to_string(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: "agent-1:0000000000000006".to_string(),
                text: "From Wardian agent agent-1: AUTO_TEST_2_DONE\r\n{\"type\":\"model\",\"content\":\"From Wardian agent agent-1: AUTO_TEST_2_DONE\"}".to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
        };

        assert!(!watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("AUTO_TEST_2_DONE"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_matches_origin_prefixed_response_after_echo() {
        let snapshot = crate::state::agent_watch::WatchSnapshot {
            cursor: "agent-1:0000000000000007".to_string(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: "agent-1:0000000000000007".to_string(),
                text: "From Wardian agent agent-1: AUTO_TEST_2_DONE\r\n{\"type\":\"model\",\"content\":\"From Wardian agent agent-1: AUTO_TEST_2_DONE\"}\r\nActual response after echo: From Wardian agent agent-1: AUTO_TEST_2_DONE".to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
        };

        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("AUTO_TEST_2_DONE"),
        ));
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
            None,
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
