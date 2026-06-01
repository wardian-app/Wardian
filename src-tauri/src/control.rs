use crate::manager;
use crate::state::{AppState, MailboxMessageDraft};
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
    AgentWorktreeMutationResponse, AgentWorktreeSummary, ApprovalAction, AskResponse,
    ControlRequest, DeliveryDetail, DeliveryErrorDetail, InteractionBodyRef, MessageInputMode,
    MessageOrigin, OkResponse, ProviderInputReadiness, ProviderReadyEvidence, QueuePolicy,
    ReplyResponse, ReplyStatus, SendMessageResponse, StructuredReply, WatchAgentSnapshot,
    WatchDeliverySnapshot, WatchEvidenceError,
};
use wardian_core::identity::{normalize_status, AgentIdentity, StatusSource};

const STRUCTURED_ASK_INLINE_MESSAGE_MAX_BYTES: usize = 4096;
const STRUCTURED_ASK_REQUESTS_DIR: &str = "requests";
const CODEX_PAYLOAD_ECHO_TIMEOUT_MS: u64 = 750;

#[cfg(windows)]
pub(crate) type ControlEndpointClaim = tokio::net::windows::named_pipe::NamedPipeServer;

#[cfg(unix)]
pub(crate) struct ControlEndpointClaim {
    listener: Option<tokio::net::UnixListener>,
    socket_path: PathBuf,
}

#[cfg(unix)]
impl Drop for ControlEndpointClaim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Run a synchronous closure inside a Tokio runtime context.
///
/// The Tauri `setup` hook runs on the main thread before any async runtime is
/// entered, but several Tokio I/O constructors (e.g. `NamedPipeServer::create`
/// on Windows, `UnixListener::bind` on Unix) register their handles with the
/// reactor and panic when called outside a runtime. When a runtime is already
/// current (e.g. inside a `#[tokio::test]` or a `tauri::async_runtime::spawn`
/// task), invoke `f` directly to avoid nesting `block_on`, which Tokio rejects.
/// Otherwise enter the Tauri-managed runtime via `block_on`. `f` is non-async,
/// so `block_on` returns synchronously.
fn run_in_tokio_runtime<R>(f: impl FnOnce() -> R) -> R {
    if tokio::runtime::Handle::try_current().is_ok() {
        f()
    } else {
        tauri::async_runtime::block_on(async { f() })
    }
}

#[cfg(windows)]
pub(crate) fn claim_control_endpoint() -> std::io::Result<ControlEndpointClaim> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = wardian_core::control::pipe_name()
        .ok_or_else(|| std::io::Error::other("could not resolve Wardian control pipe"))?;

    run_in_tokio_runtime(|| {
        ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
    })
}

#[cfg(unix)]
pub(crate) fn claim_control_endpoint() -> std::io::Result<ControlEndpointClaim> {
    use std::os::unix::net::UnixStream;
    use tokio::net::UnixListener;

    let socket_path = wardian_core::control::socket_path()
        .ok_or_else(|| std::io::Error::other("could not resolve Wardian control socket"))?;
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    run_in_tokio_runtime(|| match UnixListener::bind(&socket_path) {
        Ok(listener) => Ok(ControlEndpointClaim {
            listener: Some(listener),
            socket_path: socket_path.clone(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&socket_path).is_ok() {
                Err(error)
            } else {
                let _ = std::fs::remove_file(&socket_path);
                UnixListener::bind(&socket_path).map(|listener| ControlEndpointClaim {
                    listener: Some(listener),
                    socket_path: socket_path.clone(),
                })
            }
        }
        Err(error) => Err(error),
    })
}

pub(crate) fn spawn_control_server(app: AppHandle, claim: ControlEndpointClaim) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_control_server(app, claim).await {
            crate::utils::logging::log_debug(&format!(
                "[Wardian] control server unavailable: {error}"
            ));
        }
    });
}

#[cfg(windows)]
async fn run_control_server(
    app: AppHandle,
    first_server: ControlEndpointClaim,
) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = wardian_core::control::pipe_name()
        .ok_or_else(|| std::io::Error::other("could not resolve Wardian control pipe"))?;

    let mut next_server = Some(first_server);
    loop {
        let server = match next_server.take() {
            Some(server) => server,
            None => ServerOptions::new().create(&pipe_name)?,
        };
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
async fn run_control_server(
    app: AppHandle,
    mut claim: ControlEndpointClaim,
) -> std::io::Result<()> {
    let listener = claim
        .listener
        .take()
        .ok_or_else(|| std::io::Error::other("Wardian control endpoint was already claimed"))?;
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

        ControlRequest::SendMessage {
            target,
            message,
            thread,
            input_mode,
            queue_policy,
            approval_action,
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
                queue_policy,
                approval_action.as_ref(),
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
            include,
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
                    include,
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
        can_delete: summary.can_delete,
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
    let normalized = normalize_worktree_lookup_path(folder);
    worktrees
        .iter()
        .find(|worktree| {
            normalize_worktree_lookup_path(&worktree.worktree_folder) == normalized
                || normalize_worktree_lookup_path(&worktree.id) == normalized
        })
        .cloned()
}

fn normalize_worktree_lookup_path(path: &str) -> String {
    let normalized = path.trim().replace('\\', "/");
    let normalized = if let Some(stripped) = normalized.strip_prefix("//?/UNC/") {
        format!("//{stripped}")
    } else if let Some(stripped) = normalized.strip_prefix("//?/") {
        stripped.to_string()
    } else {
        normalized
    };
    let normalized = normalized.trim_end_matches('/').to_string();

    #[cfg(windows)]
    {
        normalized.to_ascii_lowercase()
    }

    #[cfg(not(windows))]
    {
        normalized
    }
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

#[allow(clippy::too_many_arguments)]
async fn deliver_message_to_target(
    app: Option<&AppHandle>,
    state: &AppState,
    target: &str,
    message: &str,
    thread: Option<&str>,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    approval_action: Option<&ApprovalAction>,
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
    let mut queued = 0usize;
    let mut failures = Vec::new();
    let mut delivery = Vec::with_capacity(session_ids.len());
    for info in target_infos {
        let outbound_message = message_with_origin(
            state,
            message,
            input_mode,
            origin,
            info.status == "action_required",
        )
        .await;
        let route = if input_mode == MessageInputMode::ApprovalAction
            || matches!(queue_policy, QueuePolicy::MailboxOnly)
        {
            decide_delivery_route(&info.status, input_mode, queue_policy, approval_action)
        } else if provider_input_has_known_not_ready_state(state, &info.uuid).await
            && !provider_idle_status_allows_live_delivery(&info, queue_policy)
        {
            match queue_policy {
                QueuePolicy::QueueIfBusy => DeliveryRoute::Mailbox {
                    runtime_state: "provider_input_not_ready",
                },
                QueuePolicy::LiveOnly => DeliveryRoute::Reject {
                    failure: "not_input_ready",
                },
                QueuePolicy::MailboxOnly => unreachable!("handled above"),
            }
        } else {
            decide_delivery_route(&info.status, input_mode, queue_policy, approval_action)
        };
        match route {
            DeliveryRoute::Mailbox { runtime_state } => {
                queued += 1;
                let queued_uuid = info.uuid.clone();
                let queued_status = info.status.clone();
                let detail = enqueue_mailbox_delivery(
                    state,
                    info,
                    outbound_message,
                    input_mode,
                    queue_policy,
                    approval_action,
                    origin,
                    runtime_state,
                )
                .await;
                record_delivery_attempt(state, &detail).await;
                if let Some(app) = app {
                    spawn_mailbox_drain_if_idle(app, &queued_uuid, &queued_status);
                }
                delivery.push(detail);
            }
            DeliveryRoute::Reject { failure } => {
                failures.push(format!("{}: {failure}", info.uuid));
                let detail = rejected_delivery_detail(info, failure, input_mode, queue_policy);
                record_delivery_attempt(state, &detail).await;
                delivery.push(detail);
            }
            DeliveryRoute::Live => match senders.get(&info.uuid).and_then(Clone::clone) {
                Some(tx) => {
                    let profile = crate::utils::delivery_profile::delivery_profile(&info.provider);
                    let delivery_lock = state.delivery_lock_for(&info.uuid).await;
                    let _delivery_guard = delivery_lock.lock().await;
                    let result = if let (MessageInputMode::ApprovalAction, Some(action)) =
                        (input_mode, approval_action)
                    {
                        submit_approval_action_via_sender(&tx, &info.provider, action)
                            .await
                            .map_err(|error| error.to_string())
                    } else {
                        let payload_cursor =
                            codex_payload_echo_cursor(state, &info.provider, &info.uuid).await;
                        match wait_for_terminal_ready_for_control_send(state, &info).await {
                            Ok(()) => {
                            let wait_uuid = info.uuid.clone();
                            let wait_provider = info.provider.clone();
                            let wait_prompt = outbound_message.clone();
                            crate::utils::terminal_input::submit_prompt_with_outcome_via_sender_after_payload(
                                &tx,
                                &outbound_message,
                                &info.provider,
                                || async move {
                                    wait_for_codex_payload_echo_before_submit(
                                        state,
                                        &wait_provider,
                                        &wait_uuid,
                                        payload_cursor.as_deref(),
                                        &wait_prompt,
                                    )
                                    .await;
                                },
                            )
                            .await
                            .map_err(|error| error.to_string())
                        }
                        Err(error) => Err(error),
                        }
                    };
                    match result {
                        Ok(outcome) => {
                            if input_mode == MessageInputMode::ApprovalAction
                                && matches!(approval_action, Some(ApprovalAction::Accept))
                            {
                                mark_approval_accept_started(app, state, &info.uuid).await;
                            }
                            delivered += 1;
                            let detail = DeliveryDetail {
                                uuid: info.uuid,
                                name: info.name,
                                provider: info.provider,
                                runtime_state: "live_pty_available".to_string(),
                                delivery_state: outcome.delivery_state,
                                input_mode,
                                queue_policy,
                                message_id: None,
                                delivery_phase: Some(outcome.delivery_phase),
                                observed_state: outcome.observed_state,
                                reason: outcome.reason,
                                profile: Some(profile.provider),
                                error: None,
                            };
                            record_delivery_attempt(state, &detail).await;
                            mark_delivered_agents_prompt_started(
                                app,
                                state,
                                std::slice::from_ref(&detail.uuid),
                            )
                            .await;
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
                                queue_policy,
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
                        queue_policy,
                    );
                    record_delivery_attempt(state, &detail).await;
                    delivery.push(detail);
                }
            },
        }
    }
    if delivered + queued == 0 {
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

fn provider_idle_status_allows_live_delivery(
    info: &DeliveryTargetInfo,
    queue_policy: QueuePolicy,
) -> bool {
    matches!(queue_policy, QueuePolicy::LiveOnly)
        && info.provider == "claude"
        && info.status == "idle"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryRoute {
    Live,
    Mailbox { runtime_state: &'static str },
    Reject { failure: &'static str },
}

fn decide_delivery_route(
    status: &str,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    approval_action: Option<&ApprovalAction>,
) -> DeliveryRoute {
    if input_mode == MessageInputMode::ApprovalAction {
        return if approval_action.is_some() && status == "action_required" {
            DeliveryRoute::Live
        } else {
            DeliveryRoute::Reject {
                failure: "not_input_ready",
            }
        };
    }
    if matches!(queue_policy, QueuePolicy::MailboxOnly) {
        return DeliveryRoute::Mailbox {
            runtime_state: "mailbox_only",
        };
    }

    match status {
        "idle" => DeliveryRoute::Live,
        "processing" => match queue_policy {
            QueuePolicy::QueueIfBusy => DeliveryRoute::Mailbox {
                runtime_state: "target_processing",
            },
            QueuePolicy::LiveOnly => DeliveryRoute::Reject {
                failure: "not_input_ready",
            },
            QueuePolicy::MailboxOnly => unreachable!("handled above"),
        },
        "action_required" => {
            if matches!(queue_policy, QueuePolicy::QueueIfBusy) && input_mode == MessageInputMode::Message
            {
                DeliveryRoute::Mailbox {
                    runtime_state: "target_action_required",
                }
            } else {
                DeliveryRoute::Reject {
                    failure: "not_input_ready",
                }
            }
        }
        "off" | "error" => match queue_policy {
            QueuePolicy::QueueIfBusy | QueuePolicy::MailboxOnly => DeliveryRoute::Mailbox {
                runtime_state: "queued_not_live",
            },
            QueuePolicy::LiveOnly => DeliveryRoute::Reject {
                failure: "target_not_live",
            },
        },
        _ => DeliveryRoute::Reject {
            failure: "not_input_ready",
        },
    }
}

fn approval_action_bytes(provider: &str, action: &ApprovalAction) -> Vec<u8> {
    match action {
        ApprovalAction::Accept => {
            if provider.eq_ignore_ascii_case("codex")
                || provider.eq_ignore_ascii_case("antigravity")
            {
                b"\r".to_vec()
            } else {
                b"y\r".to_vec()
            }
        }
        ApprovalAction::Reject => {
            if provider.eq_ignore_ascii_case("codex") {
                b"\x1b".to_vec()
            } else {
                b"n\r".to_vec()
            }
        }
        ApprovalAction::Select { option } => {
            let mut bytes = option.as_bytes().to_vec();
            bytes.push(b'\r');
            bytes
        }
        ApprovalAction::FreeText { text } => {
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(b'\r');
            bytes
        }
    }
}

async fn submit_approval_action_via_sender(
    tx: &tokio::sync::mpsc::Sender<Vec<u8>>,
    provider: &str,
    action: &ApprovalAction,
) -> Result<crate::utils::delivery_transaction::TerminalDeliveryOutcome, String> {
    let bytes = approval_action_bytes(provider, action);
    tx.send(bytes)
        .await
        .map_err(|_| "input channel closed".to_string())?;
    Ok(
        crate::utils::delivery_transaction::TerminalDeliveryOutcome {
            delivery_state: "submit_sent_unverified".to_string(),
            delivery_phase: "approval_key_sent".to_string(),
            observed_state: Some("bytes_sent".to_string()),
            reason: None,
        },
    )
}

async fn mark_approval_accept_started(app: Option<&AppHandle>, state: &AppState, session_id: &str) {
    let Some(app) = app else {
        return;
    };
    let current_status = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .map(|agent| agent.current_status.clone())
    };
    if let Some(current_status) = current_status {
        manager::set_agent_status(app, session_id, &current_status, "Processing...");
    }
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_mailbox_delivery(
    state: &AppState,
    info: DeliveryTargetInfo,
    body: String,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    approval_action: Option<&ApprovalAction>,
    origin: Option<&MessageOrigin>,
    runtime_state: &str,
) -> DeliveryDetail {
    let mut mailbox = state.mailbox.lock().await;
    let record = mailbox.enqueue(MailboxMessageDraft {
        target_session_id: info.uuid.clone(),
        body,
        input_mode,
        queue_policy,
        approval_action: approval_action.cloned(),
        origin: origin.cloned(),
    });

    DeliveryDetail {
        uuid: info.uuid,
        name: info.name,
        provider: info.provider,
        runtime_state: runtime_state.to_string(),
        delivery_state: "queued".to_string(),
        input_mode,
        queue_policy,
        message_id: Some(record.id),
        delivery_phase: Some("queued".to_string()),
        observed_state: None,
        reason: Some("target was not safe for live delivery".to_string()),
        profile: None,
        error: None,
    }
}

async fn message_with_origin(
    state: &AppState,
    message: &str,
    input_mode: MessageInputMode,
    origin: Option<&MessageOrigin>,
    allow_bare_approval_response: bool,
) -> String {
    if matches!(
        input_mode,
        MessageInputMode::Command | MessageInputMode::ApprovalAction
    ) {
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
    if provider_input_current_state(state, &info.uuid).await == Some(ProviderInputReadiness::Ready)
    {
        return Ok(());
    }

    if info.provider == "opencode" {
        wait_for_opencode_terminal_ready(state, &info.uuid, 15_000).await
    } else if info.provider == "codex" {
        wait_for_terminal_output(state, &info.uuid, 15_000, codex_output_has_ready_prompt).await
    } else if info.provider == "claude" {
        if current_agent_status_is_idle(state, &info.uuid).await? {
            Ok(())
        } else {
            wait_for_terminal_output(state, &info.uuid, 15_000, claude_output_has_ready_prompt)
                .await
        }
    } else if info.provider == "gemini" {
        wait_for_terminal_output(state, &info.uuid, 15_000, gemini_output_has_ready_prompt).await
    } else if info.provider == "antigravity" {
        wait_for_terminal_output(
            state,
            &info.uuid,
            15_000,
            antigravity_output_has_ready_prompt,
        )
        .await
    } else if provider_input_has_known_not_ready_state(state, &info.uuid).await {
        Err(format!("Agent {} provider input is not ready", info.uuid))
    } else if current_agent_status_is_idle(state, &info.uuid).await? {
        Ok(())
    } else {
        Err(format!("Agent {} is not idle", info.uuid))
    }
}

async fn provider_input_has_known_not_ready_state(state: &AppState, session_id: &str) -> bool {
    provider_input_current_state(state, session_id)
        .await
        .is_some_and(|input_state| input_state != ProviderInputReadiness::Ready)
}

async fn provider_input_current_state(
    state: &AppState,
    session_id: &str,
) -> Option<ProviderInputReadiness> {
    let input = state.interactions.provider_input_state(session_id).await?;
    let current_generation = state
        .interactions
        .current_provider_input_generation(session_id)
        .await?;
    (input.generation == current_generation).then_some(input.state)
}

async fn provider_input_blocks_mailbox_drain(state: &AppState, session_id: &str) -> bool {
    matches!(
        provider_input_current_state(state, session_id).await,
        Some(
            ProviderInputReadiness::Busy
                | ProviderInputReadiness::ActionRequired
                | ProviderInputReadiness::Unavailable
        )
    )
}

async fn record_provider_ready_evidence(
    state: &AppState,
    session_id: &str,
    evidence: ProviderReadyEvidence,
) {
    let generation = state
        .interactions
        .provider_input_state(session_id)
        .await
        .filter(|input| input.state != ProviderInputReadiness::ActionRequired)
        .map(|input| input.generation)
        .unwrap_or(0);
    state
        .interactions
        .record_provider_input_state(
            session_id,
            generation,
            ProviderInputReadiness::Ready,
            Some(evidence),
        )
        .await;
}

async fn current_agent_status_is_idle(state: &AppState, session_id: &str) -> Result<bool, String> {
    let status = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .ok_or_else(|| format!("Agent {} not found or is off", session_id))?
            .current_status
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    };
    Ok(wardian_core::identity::normalize_status(&status) == "idle")
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
            record_provider_ready_evidence(state, session_id, ProviderReadyEvidence::TitleDetected)
                .await;
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
        if !current_agent_status_is_idle(state, session_id).await? {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        }
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
            record_provider_ready_evidence(
                state,
                session_id,
                ProviderReadyEvidence::PromptDetected,
            )
            .await;
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(format!(
        "Timed out waiting for {} terminal output to become ready",
        session_id
    ))
}

async fn codex_payload_echo_cursor(
    state: &AppState,
    provider: &str,
    session_id: &str,
) -> Option<String> {
    if provider != "codex" {
        return None;
    }

    let watch_state = {
        let agents = state.agents.lock().await;
        agents.get(session_id)?.watch_state.clone()
    };
    watch_state.lock().ok().map(|guard| guard.latest_cursor())
}

async fn wait_for_codex_payload_echo_before_submit(
    state: &AppState,
    provider: &str,
    session_id: &str,
    since_cursor: Option<&str>,
    prompt: &str,
) {
    if provider != "codex" {
        return;
    }

    let Some(since_cursor) = since_cursor else {
        return;
    };

    if let Err(error) =
        wait_for_codex_prompt_echo_since(
            state,
            session_id,
            since_cursor,
            prompt,
            CODEX_PAYLOAD_ECHO_TIMEOUT_MS,
        )
        .await
    {
        manager::log_debug(&format!(
            "[Wardian] [{session_id}] Codex prompt echo wait before submit did not complete: {error}"
        ));
    }
}

async fn wait_for_codex_prompt_echo_since(
    state: &AppState,
    session_id: &str,
    since_cursor: &str,
    prompt: &str,
    timeout_ms: u64,
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
            .snapshot_since(Some(since_cursor), Some(16 * 1024))
            .map(|snapshot| snapshot.output.text)
            .map_err(|error| format!("watch state error: {}", error.code()))?;

        if codex_output_has_prompt_echo(&output, prompt) {
            return Ok(());
        }

        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    Err(format!(
        "Timed out waiting for {} Codex prompt echo before submit",
        session_id
    ))
}

fn codex_output_has_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    let mut trailing_metadata_lines = 0usize;
    for line in cleaned.lines().rev().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('›') {
            return true;
        }
        if trailing_metadata_lines < 3 && codex_ready_prompt_trailing_metadata_line(line) {
            trailing_metadata_lines += 1;
            continue;
        }
        return false;
    }
    false
}

fn codex_output_has_prompt_echo(output: &str, prompt: &str) -> bool {
    let Some(token) = codex_prompt_echo_token(prompt) else {
        return false;
    };
    normalize_codex_prompt_echo_text(output).contains(&token)
}

fn codex_prompt_echo_token(prompt: &str) -> Option<String> {
    let first_line = prompt.lines().map(str::trim).find(|line| !line.is_empty())?;
    let prefix: String = first_line.chars().take(96).collect();
    let token = normalize_codex_prompt_echo_text(&prefix);
    (!token.is_empty()).then_some(token)
}

fn normalize_codex_prompt_echo_text(text: &str) -> String {
    strip_ansi_controls(text)
        .replace('\r', "\n")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn codex_ready_prompt_trailing_metadata_line(line: &str) -> bool {
    if line.contains('•') {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    lower.starts_with("gpt-") && (line.contains('·') || lower.contains("context"))
}

fn claude_output_has_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    let mut trailing_metadata_lines = 0usize;
    for line in cleaned.lines().rev().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('❯') {
            return true;
        }
        if trailing_metadata_lines < 4 && claude_ready_prompt_trailing_metadata_line(line) {
            trailing_metadata_lines += 1;
            continue;
        }
        return false;
    }
    false
}

fn claude_ready_prompt_trailing_metadata_line(line: &str) -> bool {
    if line.contains('⏵') {
        return true;
    }
    line.chars()
        .all(|ch| ch == '─' || ch == '-' || ch.is_whitespace())
}

fn gemini_output_has_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    let tail = cleaned
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12);
    for line in tail {
        if line.contains("Type your message or @path/to/file") {
            return true;
        }
    }
    false
}

fn antigravity_output_has_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    let lines = cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate().rev().take(16) {
        if *line != ">" {
            continue;
        }
        let has_ready_footer = lines
            .iter()
            .skip(index + 1)
            .take(4)
            .any(|line| antigravity_ready_prompt_footer_line(line));
        if has_ready_footer {
            return true;
        }
    }
    false
}

fn antigravity_ready_prompt_footer_line(line: &str) -> bool {
    line.contains("Press up to edit queued messages") || line.contains("? for shortcuts")
}

async fn mark_delivered_agents_prompt_started(
    app: Option<&AppHandle>,
    state: &AppState,
    session_ids: &[String],
) {
    if session_ids.is_empty() {
        return;
    }

    for session_id in session_ids {
        state
            .interactions
            .start_provider_input_generation(session_id, ProviderInputReadiness::Busy, None)
            .await;
        let agents = state.agents.lock().await;
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
    let sender_session_id =
        origin.map(|MessageOrigin::WardianAgent { session_id }| session_id.clone());
    let body_ref = structured_delivery
        .body_file
        .as_ref()
        .map(|path| InteractionBodyRef::File {
            path: path.display().to_string(),
        })
        .unwrap_or_else(|| InteractionBodyRef::Inline {
            body: message.to_string(),
        });
    let task = state
        .interactions
        .create_task_with_id(
            request_id.clone(),
            sender_session_id,
            target_uuid.clone(),
            body_ref,
        )
        .await;
    let mut payload = serde_json::json!({
        "request_id": task.id,
        "target_session_id": target_uuid,
        "status": "pending",
        "created_at": task.created_at,
    });
    if let Some(body_file) = structured_delivery.body_file.as_deref() {
        if let Some(payload) = payload.as_object_mut() {
            payload.insert(
                "body_file".to_string(),
                serde_json::Value::String(body_file.display().to_string()),
            );
        }
    }
    push_watch_event_for_agent(&state, &target_uuid, "request", payload).await?;
    let delivery = match deliver_message_to_target(
        Some(app),
        &state,
        target,
        &structured_delivery.prompt,
        thread,
        MessageInputMode::Message,
        QueuePolicy::QueueIfBusy,
        None,
        origin,
    )
    .await
    {
        Ok(delivery) => delivery,
        Err(error) => {
            return Err(error);
        }
    };
    let reply = match wait_for_structured_reply(&state, &request_id, timeout).await {
        Ok(reply) => reply,
        Err(error) => {
            return Err(error);
        }
    };
    let fallback_agent = ask_fallback_agent_snapshot(&state, &target_uuid, target).await;
    let watch_result = structured_ask_watch_response(
        &state,
        &target_uuid,
        watch_state,
        &initial_cursor,
        tail_bytes,
    )
    .await;
    let response = build_ask_response_with_watch_result(
        request_id.clone(),
        target.to_string(),
        delivery,
        reply,
        fallback_agent,
        watch_result,
    );
    ok_json(&response)
}

async fn structured_ask_watch_response(
    state: &AppState,
    target_uuid: &str,
    watch_state: Arc<Mutex<crate::state::AgentWatchState>>,
    initial_cursor: &str,
    tail_bytes: Option<usize>,
) -> Result<AgentWatchResponse, ControlError> {
    let snapshot = watch_state
        .lock()
        .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?
        .snapshot_since(Some(initial_cursor), tail_bytes)
        .map_err(control_error_from_watch_state)?;
    let agent = watch_agent_snapshot(state, target_uuid).await?;

    Ok(build_agent_watch_response(
        agent,
        snapshot,
        &WatchIncludes::from_values(&[
            "events".to_string(),
            "transcript".to_string(),
            "output".to_string(),
            "delivery".to_string(),
        ]),
    ))
}

fn build_ask_response_with_watch_result(
    request_id: String,
    target: String,
    delivery: Vec<DeliveryDetail>,
    reply: StructuredReply,
    fallback_agent: WatchAgentSnapshot,
    watch_result: Result<AgentWatchResponse, ControlError>,
) -> AskResponse {
    match watch_result {
        Ok(watch) => AskResponse {
            schema: wardian_core::control::CONTROL_SCHEMA,
            ok: true,
            request_id,
            target,
            delivery,
            reply,
            watch,
            watch_error: None,
        },
        Err(error) => AskResponse {
            schema: wardian_core::control::CONTROL_SCHEMA,
            ok: true,
            request_id,
            target,
            delivery,
            reply,
            watch: minimal_ask_watch_response(fallback_agent),
            watch_error: Some(WatchEvidenceError {
                code: error.code().to_string(),
                message: error.to_string(),
            }),
        },
    }
}

fn minimal_ask_watch_response(agent: WatchAgentSnapshot) -> AgentWatchResponse {
    let cursor = format!("{}:degraded", agent.uuid);
    AgentWatchResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        agent,
        cursor: cursor.clone(),
        events: Vec::new(),
        output: wardian_core::control::WatchOutput {
            cursor,
            text: String::new(),
            truncated: false,
            omitted_bytes: 0,
        },
        transcript: None,
        raw_output: None,
        delivery: WatchDeliverySnapshot {
            delivery: Vec::new(),
        },
    }
}

async fn ask_fallback_agent_snapshot(
    state: &AppState,
    target_uuid: &str,
    target: &str,
) -> WatchAgentSnapshot {
    watch_agent_snapshot(state, target_uuid)
        .await
        .unwrap_or_else(|_| WatchAgentSnapshot {
            uuid: target_uuid.to_string(),
            name: target.to_string(),
            provider: String::new(),
            status: "unknown".to_string(),
            last_status_at: None,
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

#[cfg(test)]
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

    if state.interactions.interaction(request_id).await.is_some() {
        let reply = state
            .interactions
            .complete_task_with_reply(
                request_id,
                source_session_id.as_deref(),
                status.clone(),
                body,
            )
            .await
            .map_err(|code| match code {
                "not_found" => {
                    ControlError::not_found(format!("ask request not found: {request_id}"))
                }
                "unauthorized" => {
                    ControlError::coded("unauthorized", "reply origin does not match ask target")
                }
                "duplicate_reply" => ControlError::coded(
                    "duplicate_reply",
                    "ask request already has a terminal reply",
                ),
                _ => ControlError::request_failed("failed to complete ask interaction"),
            })?;

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
        return Ok(reply);
    }

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
        if state.interactions.interaction(request_id).await.is_some() {
            if let Some(reply) = state.interactions.structured_reply(request_id).await {
                return Ok(reply);
            }
            if started.elapsed() >= timeout {
                return Err(ControlError::watch_timeout("structured reply timed out")
                    .with_details(serde_json::json!({
                        "request_id": request_id,
                        "until": "reply",
                    })));
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            tokio::time::sleep(remaining.min(Duration::from_millis(25))).await;
            continue;
        }

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
    let includes = WatchIncludes::from_values(&options.include);

    ok_json(&build_agent_watch_response(agent, snapshot, &includes))
}

struct AgentWatchControlOptions {
    since: Option<String>,
    until: Option<String>,
    include: Vec<String>,
    tail_bytes: Option<usize>,
    follow: bool,
    timeout_ms: Option<u64>,
    output_echo_guard: Option<String>,
}

#[derive(Debug, Clone)]
struct WatchIncludes {
    events: bool,
    output: bool,
    transcript: bool,
    raw_output: bool,
    delivery: bool,
}

impl WatchIncludes {
    fn from_values(values: &[String]) -> Self {
        let values = if values.is_empty() {
            vec![
                "status".to_string(),
                "transcript".to_string(),
                "output".to_string(),
                "delivery".to_string(),
            ]
        } else {
            values.to_vec()
        };

        Self {
            events: values.iter().any(|value| value == "events"),
            output: values.iter().any(|value| value == "output"),
            transcript: values.iter().any(|value| value == "transcript"),
            raw_output: values.iter().any(|value| value == "raw_output"),
            delivery: values.iter().any(|value| value == "delivery"),
        }
    }
}

fn build_agent_watch_response(
    agent: WatchAgentSnapshot,
    snapshot: crate::state::agent_watch::WatchSnapshot,
    includes: &WatchIncludes,
) -> AgentWatchResponse {
    let cursor = snapshot.cursor.clone();
    let events = if includes.events {
        snapshot.events.clone()
    } else {
        Vec::new()
    };
    let delivery = if includes.delivery {
        delivery_snapshot_from_events(&snapshot.events)
    } else {
        WatchDeliverySnapshot {
            delivery: Vec::new(),
        }
    };
    let empty_output = || wardian_core::control::WatchOutput {
        cursor: cursor.clone(),
        text: String::new(),
        truncated: false,
        omitted_bytes: 0,
    };
    AgentWatchResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        agent,
        cursor: cursor.clone(),
        events,
        output: if includes.output {
            snapshot.output
        } else {
            empty_output()
        },
        transcript: includes.transcript.then_some(snapshot.transcript),
        raw_output: includes.raw_output.then_some(snapshot.raw_output),
        delivery,
    }
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
        WatchCondition::OutputContains(token) => [
            snapshot.transcript.latest_text.as_str(),
            snapshot.output.text.as_str(),
            snapshot.raw_output.text.as_str(),
        ]
        .into_iter()
        .filter(|text| text.contains(token))
        .any(|text| !output_match_is_prompt_echo_only(token, text, output_echo_guard)),
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
    queue_policy: QueuePolicy,
) -> DeliveryDetail {
    DeliveryDetail {
        uuid: info.uuid,
        name: info.name,
        provider: info.provider,
        runtime_state: runtime_state.to_string(),
        delivery_state: "failed".to_string(),
        input_mode,
        queue_policy,
        message_id: None,
        delivery_phase: None,
        observed_state: None,
        reason: None,
        profile: None,
        error: Some(DeliveryErrorDetail {
            code: error_code.to_string(),
            message: error_message.into(),
        }),
    }
}

fn rejected_delivery_detail(
    info: DeliveryTargetInfo,
    failure: &str,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
) -> DeliveryDetail {
    DeliveryDetail {
        uuid: info.uuid,
        name: info.name,
        provider: info.provider,
        runtime_state: "live_delivery_rejected".to_string(),
        delivery_state: failure.to_string(),
        input_mode,
        queue_policy,
        message_id: None,
        delivery_phase: None,
        observed_state: None,
        reason: None,
        profile: None,
        error: Some(DeliveryErrorDetail {
            code: failure.to_string(),
            message: failure.to_string(),
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

pub(crate) fn spawn_mailbox_drain_if_idle(
    app: &AppHandle,
    session_id: &str,
    observed_status: &str,
) {
    if normalize_status(observed_status) != "idle" {
        return;
    }

    let app = app.clone();
    let session_id = session_id.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _ = drain_next_mailbox_message_for_idle_agent(Some(&app), &state, &session_id).await;
    });
}

fn spawn_delayed_mailbox_drain_retry(app: Option<&AppHandle>, session_id: &str) {
    let Some(app) = app.cloned() else {
        return;
    };
    let session_id = session_id.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2_000)).await;
        let state = app.state::<AppState>();
        let _ = drain_next_mailbox_message_for_idle_agent(Some(&app), &state, &session_id).await;
    });
}

enum MailboxSubmitError {
    RetrySafe(String),
    Terminal(crate::utils::delivery_transaction::TerminalDeliveryError),
}

impl MailboxSubmitError {
    fn message(&self) -> String {
        match self {
            MailboxSubmitError::RetrySafe(message) => message.clone(),
            MailboxSubmitError::Terminal(error) => error.to_string(),
        }
    }

    fn phase(&self) -> Option<&'static str> {
        match self {
            MailboxSubmitError::RetrySafe(_) => None,
            MailboxSubmitError::Terminal(error) => Some(error.phase),
        }
    }

    fn retry_safe(&self) -> bool {
        match self {
            MailboxSubmitError::RetrySafe(_) => true,
            MailboxSubmitError::Terminal(error) => error.retry_safe,
        }
    }
}

async fn drain_next_mailbox_message_for_idle_agent(
    app: Option<&AppHandle>,
    state: &AppState,
    session_id: &str,
) -> Result<Option<DeliveryDetail>, ControlError> {
    let delivery_lock = state.delivery_lock_for(session_id).await;
    let _delivery_guard = delivery_lock.lock().await;
    let info = delivery_target_infos(state, &[session_id.to_string()])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ControlError::not_found(format!("agent not found: {session_id}")))?;
    if info.status != "idle" {
        return Ok(None);
    }
    if provider_input_blocks_mailbox_drain(state, session_id).await {
        return Ok(None);
    }

    let record = {
        let mut mailbox = state.mailbox.lock().await;
        mailbox.take_next_pending_for_target(session_id)
    };
    let Some(record) = record else {
        return Ok(None);
    };

    let sender = state
        .input_senders
        .read()
        .map_err(|_| ControlError::request_failed("input_senders lock poisoned"))?
        .get(session_id)
        .cloned();
    let profile = crate::utils::delivery_profile::delivery_profile(&info.provider);
    let payload_cursor = codex_payload_echo_cursor(state, &info.provider, session_id).await;
    let detail = match sender {
        Some(tx) => {
            let result = match wait_for_terminal_ready_for_control_send(state, &info).await {
                Ok(()) => {
                    let submit_started = DeliveryDetail {
                        uuid: info.uuid.clone(),
                        name: info.name.clone(),
                        provider: info.provider.clone(),
                        runtime_state: "mailbox_drain".to_string(),
                        delivery_state: "submit_started".to_string(),
                        input_mode: record.input_mode,
                        queue_policy: record.queue_policy,
                        message_id: Some(record.id.clone()),
                        delivery_phase: Some("payload_sent".to_string()),
                        observed_state: Some("payload_sent".to_string()),
                        reason: None,
                        profile: Some(profile.provider.clone()),
                        error: None,
                    };
                    let wait_provider = info.provider.clone();
                    let wait_uuid = session_id.to_string();
                    let wait_prompt = record.body.clone();
                    crate::utils::terminal_input::submit_prompt_with_outcome_via_sender_after_payload(
                        &tx,
                        &record.body,
                        &info.provider,
                        || {
                            let submit_started = submit_started.clone();
                            async move {
                                record_delivery_attempt(state, &submit_started).await;
                                wait_for_codex_payload_echo_before_submit(
                                    state,
                                    &wait_provider,
                                    &wait_uuid,
                                    payload_cursor.as_deref(),
                                    &wait_prompt,
                                )
                                .await;
                            }
                        },
                    )
                    .await
                    .map_err(MailboxSubmitError::Terminal)
                }
                Err(error) => Err(MailboxSubmitError::RetrySafe(error)),
            };
            match result {
                Ok(outcome) => {
                    state.mailbox.lock().await.mark_delivered(&record.id);
                    let detail = DeliveryDetail {
                        uuid: info.uuid,
                        name: info.name,
                        provider: info.provider,
                        runtime_state: "mailbox_drain".to_string(),
                        delivery_state: outcome.delivery_state,
                        input_mode: record.input_mode,
                        queue_policy: record.queue_policy,
                        message_id: Some(record.id),
                        delivery_phase: Some(outcome.delivery_phase),
                        observed_state: outcome.observed_state,
                        reason: outcome.reason,
                        profile: Some(profile.provider),
                        error: None,
                    };
                    record_delivery_attempt(state, &detail).await;
                    mark_delivered_agents_prompt_started(app, state, &[session_id.to_string()])
                        .await;
                    detail
                }
                Err(error) => {
                    if error.retry_safe() {
                        state.mailbox.lock().await.mark_pending(&record.id);
                    } else {
                        state.mailbox.lock().await.mark_failed(&record.id);
                    }
                    let mut detail = failed_delivery_detail(
                        info,
                        "mailbox_drain",
                        "send_failed",
                        error.message(),
                        record.input_mode,
                        record.queue_policy,
                    );
                    detail.message_id = Some(record.id);
                    detail.delivery_phase = Some(
                        error
                            .phase()
                            .unwrap_or(if error.retry_safe() {
                                "queued"
                            } else {
                                "terminal_state_unknown"
                            })
                            .to_string(),
                    );
                    detail.reason = Some(if error.retry_safe() {
                        "queued message remains pending for retry".to_string()
                    } else {
                        "queued message marked failed because terminal state is partial or unknown"
                            .to_string()
                    });
                    record_delivery_attempt(state, &detail).await;
                    if error.retry_safe() {
                        spawn_delayed_mailbox_drain_retry(app, session_id);
                    }
                    detail
                }
            }
        }
        None => {
            state.mailbox.lock().await.mark_pending(&record.id);
            let mut detail = failed_delivery_detail(
                info,
                "mailbox_drain",
                "no_input_channel",
                "missing sender",
                record.input_mode,
                record.queue_policy,
            );
            detail.message_id = Some(record.id);
            detail.delivery_phase = Some("queued".to_string());
            detail.reason = Some("queued message remains pending for retry".to_string());
            record_delivery_attempt(state, &detail).await;
            detail
        }
    };

    Ok(Some(detail))
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
    use wardian_core::models::AgentConfig;

    /// Regression test for the silent release-build crash where `claim_control_endpoint`
    /// was called from Tauri's `setup` hook (no Tokio runtime context), causing
    /// `tokio::net::windows::named_pipe::ServerOptions::create` to panic with
    /// "there is no reactor running". This test runs as a plain `#[test]` — *not*
    /// `#[tokio::test]` — so the absence of an ambient runtime mirrors the real
    /// setup-hook environment. The claim must succeed without panicking.
    #[test]
    fn control_endpoint_claim_succeeds_without_ambient_tokio_runtime() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", temp.path());

        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "test precondition: no Tokio runtime must be ambient on this thread, \
             otherwise we are not exercising the setup-hook code path"
        );

        let claim =
            claim_control_endpoint().expect("claim must not panic or fail outside a runtime");
        drop(claim);

        std::env::remove_var("WARDIAN_HOME");
    }

    #[tokio::test]
    async fn control_endpoint_claim_is_exclusive_for_current_home() {
        let _guard = crate::utils::wardian_test_env_lock();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", temp.path());

        let first = claim_control_endpoint().expect("first endpoint claim");
        let second = match claim_control_endpoint() {
            Ok(_) => panic!("second claim should fail"),
            Err(error) => error,
        };

        assert!(
            matches!(
                second.kind(),
                std::io::ErrorKind::AlreadyExists
                    | std::io::ErrorKind::AddrInUse
                    | std::io::ErrorKind::PermissionDenied
            ),
            "unexpected endpoint claim error: {second}"
        );

        drop(first);
        std::env::remove_var("WARDIAN_HOME");
    }

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

    fn expected_terminal_chunks(provider: &str, prompt: &str) -> Vec<Vec<u8>> {
        let chunks =
            crate::utils::terminal_input::provider_submit_chunks(provider, prompt).unwrap();
        assert_eq!(chunks.len(), 2);
        chunks
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

        assert_eq!(chunks[0], b"\x1b[200~hello\nworld\x1b[201~".to_vec());
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
        assert!(codex_output_has_ready_prompt(
            "\r\n› Explain this codebase\r\n\r\n  gpt-5.5 high · Context 100% left · C:\\projects\\example\r\n"
        ));
        assert!(codex_output_has_ready_prompt(
            "\r\n› Working on test coverage\r\n"
        ));
        assert!(codex_output_has_ready_prompt(
            "\r\n› Explain this codebase\r\n\r\n  gpt-5.5 high · Context 100% left · C:\\projects\\sample\r\n"
        ));
        assert!(!codex_output_has_ready_prompt("Booting MCP server"));
    }

    #[test]
    fn codex_ready_prompt_ignores_stale_prompt_marker_when_latest_screen_is_busy() {
        assert!(!codex_output_has_ready_prompt(
            "\r\n› Previous prompt\r\nProcessing request\r\nWorking...\r\n"
        ));
        assert!(!codex_output_has_ready_prompt(
            "\r\n› Previous prompt\r\nThinking about the request\r\n"
        ));
        assert!(!codex_output_has_ready_prompt(
            "\r\n› Previous prompt\r\nFinal response: complete\r\n"
        ));
        assert!(!codex_output_has_ready_prompt(
            "\r\n› Previous prompt\r\nFinal response: Codex context is initialized\r\n"
        ));
        assert!(!codex_output_has_ready_prompt(
            "\r\n› Previous prompt\r\nFinal response: gpt-5 · context window\r\n"
        ));
        assert!(!codex_output_has_ready_prompt(
            "\r\n› Previous prompt\r\n  gpt-5.5 high · Context 100% left · D:\\Development\\Wardian• Working...\r\n"
        ));
    }

    #[test]
    fn codex_prompt_echo_detects_payload_visible_after_send() {
        assert!(codex_output_has_prompt_echo(
            "\r\n› From Test: Ping. Reply with exactly: pong\r\n\r\n  Wardian request id: ask_123\r\n",
            "From Test: Ping. Reply with exactly: pong\n\nWardian request id: ask_123"
        ));
    }

    #[test]
    fn codex_prompt_echo_rejects_output_without_current_payload() {
        assert!(!codex_output_has_prompt_echo(
            "\r\n› Explain this codebase\r\n\r\n  gpt-5.5 high · Context 100% left · ~\r\n",
            "From Test: Ping. Reply with exactly: pong\n\nWardian request id: ask_123"
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
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\x1b[13u".to_vec());
    }

    #[test]
    fn delivery_route_queues_processing_message_when_queue_if_busy() {
        let route = decide_delivery_route(
            "processing",
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Mailbox {
                runtime_state: "target_processing"
            }
        );
    }

    #[test]
    fn delivery_route_rejects_processing_message_when_live_only() {
        let route = decide_delivery_route(
            "processing",
            MessageInputMode::Message,
            QueuePolicy::LiveOnly,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Reject {
                failure: "not_input_ready"
            }
        );
    }

    #[test]
    fn delivery_route_queues_action_required_message_when_queue_if_busy() {
        let route = decide_delivery_route(
            "action_required",
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Mailbox {
                runtime_state: "target_action_required"
            }
        );
    }

    #[test]
    fn delivery_route_sends_approval_action_when_action_required() {
        let approval_action = ApprovalAction::Accept;
        let route = decide_delivery_route(
            "action_required",
            MessageInputMode::ApprovalAction,
            QueuePolicy::QueueIfBusy,
            Some(&approval_action),
        );

        assert_eq!(
            route,
            DeliveryRoute::Live
        );
    }

    #[test]
    fn delivery_route_rejects_approval_action_without_action_required_status() {
        let route = decide_delivery_route(
            "idle",
            MessageInputMode::ApprovalAction,
            QueuePolicy::LiveOnly,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Reject {
                failure: "not_input_ready"
            }
        );
    }

    #[test]
    fn delivery_route_rejects_idle_approval_action() {
        let approval_action = ApprovalAction::Accept;
        let route = decide_delivery_route(
            "idle",
            MessageInputMode::ApprovalAction,
            QueuePolicy::LiveOnly,
            Some(&approval_action),
        );

        assert_eq!(
            route,
            DeliveryRoute::Reject {
                failure: "not_input_ready"
            }
        );
    }

    #[test]
    fn delivery_route_rejects_mailbox_only_approval_action_when_not_action_required() {
        let approval_action = ApprovalAction::Accept;
        let route = decide_delivery_route(
            "processing",
            MessageInputMode::ApprovalAction,
            QueuePolicy::MailboxOnly,
            Some(&approval_action),
        );

        assert_eq!(
            route,
            DeliveryRoute::Reject {
                failure: "not_input_ready"
            }
        );
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
    fn worktree_by_folder_matches_normalized_folder_or_id() {
        let worktrees = vec![AgentWorktreeSummary {
            id: "C:/repo/worktrees/review".to_string(),
            name: "review".to_string(),
            source_folder: "C:/repo".to_string(),
            worktree_folder: "C:/repo/worktrees/review".to_string(),
            member_agent_ids: vec!["agent-1".to_string()],
            can_delete: false,
        }];

        let matched = worktree_by_folder(&worktrees, "C:\\repo\\worktrees\\review").unwrap();

        assert_eq!(matched.id, "C:/repo/worktrees/review");
    }

    #[test]
    fn worktree_by_folder_matches_windows_case_and_trailing_slash_variants() {
        let worktrees = vec![AgentWorktreeSummary {
            id: "C:/repo/worktrees/review".to_string(),
            name: "review".to_string(),
            source_folder: "C:/repo".to_string(),
            worktree_folder: "C:/repo/worktrees/review".to_string(),
            member_agent_ids: vec!["agent-1".to_string()],
            can_delete: false,
        }];

        let matched = worktree_by_folder(&worktrees, "c:\\repo\\worktrees\\review\\");

        if cfg!(windows) {
            assert!(matched.is_some());
        } else {
            assert!(matched.is_none());
        }
    }

    #[test]
    fn worktree_for_member_returns_member_summary() {
        let worktrees = vec![AgentWorktreeSummary {
            id: "C:/repo/worktrees/review".to_string(),
            name: "review".to_string(),
            source_folder: "C:/repo".to_string(),
            worktree_folder: "C:/repo/worktrees/review".to_string(),
            member_agent_ids: vec!["agent-1".to_string(), "agent-2".to_string()],
            can_delete: false,
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
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();

        let expected = expected_terminal_chunks("codex", "hello");
        assert_eq!(rx.recv().await.unwrap(), expected[0]);
        assert_eq!(rx.recv().await.unwrap(), expected[1]);
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            assert_eq!(agent.current_status.lock().unwrap().as_str(), "Idle");
            assert_eq!(*agent.query_count.lock().unwrap(), 1);
        }
    }

    #[tokio::test]
    async fn codex_ready_prompt_is_not_ready_while_agent_is_processing() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Processing".to_string();
            agent
                .watch_state
                .lock()
                .unwrap()
                .push_output(b"\r\n\x1b[1m\xe2\x80\xba\x1b[22m Ready");
        }

        let result =
            wait_for_terminal_output(&state, "agent-1", 1, codex_output_has_ready_prompt).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn message_delivery_prefixes_agent_origin_with_sender_name() {
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
            "check this",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
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
    async fn command_delivery_keeps_origin_unattributed_and_records_input_mode() {
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

        let delivery = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "/goal test",
            None,
            MessageInputMode::Command,
            QueuePolicy::QueueIfBusy,
            None,
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
    async fn message_delivery_queues_bare_approval_responses_when_action_required() {
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

        let delivery = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "y",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
        )
        .await
        .unwrap();

        assert!(rx.try_recv().is_err());
        assert_eq!(delivery[0].runtime_state, "target_action_required");
        assert_eq!(delivery[0].delivery_state, "queued");
        let queued = state.mailbox.lock().await.list_for_target("agent-1");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].body, "y");
    }

    #[tokio::test]
    async fn approval_action_delivery_sends_provider_approval_key() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Action Needed".to_string();
        }
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
            "",
            None,
            MessageInputMode::ApprovalAction,
            QueuePolicy::QueueIfBusy,
            Some(&ApprovalAction::Accept),
            None,
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
        assert_eq!(delivery[0].runtime_state, "live_pty_available");
        assert_eq!(delivery[0].delivery_state, "submit_sent_unverified");
        assert_eq!(
            delivery[0].delivery_phase.as_deref(),
            Some("approval_key_sent")
        );
    }

    #[tokio::test]
    async fn mailbox_drain_submits_next_pending_message_when_target_is_idle() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Processing".to_string();
            agent
                .watch_state
                .lock()
                .unwrap()
                .push_output(b"\r\n\x1b[1m\xe2\x80\xba\x1b[22m Ready");
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let queued = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drained message");

        let expected = expected_terminal_chunks("codex", "queued work");
        assert_eq!(rx.recv().await.unwrap(), expected[0]);
        assert_eq!(rx.recv().await.unwrap(), expected[1]);
        assert_eq!(drained.runtime_state, "mailbox_drain");
        assert_eq!(drained.delivery_state, "submit_sent_unverified");
        assert_eq!(drained.message_id.as_deref(), Some(message_id.as_str()));
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            let snapshot = agent
                .watch_state
                .lock()
                .unwrap()
                .snapshot_since(None, None)
                .unwrap();
            assert!(snapshot.events.iter().any(|event| {
                event.kind == "delivery"
                    && event.payload["delivery_state"] == "submit_started"
                    && event.payload["message_id"] == message_id.as_str()
            }));
        }
        let records = state.mailbox.lock().await.list_for_target("agent-1");
        assert_eq!(
            records[0].status,
            crate::state::MailboxMessageStatus::Delivered
        );
        assert_eq!(
            records[0].phase,
            crate::state::MailboxDeliveryPhase::Terminal
        );
    }

    #[tokio::test]
    async fn provider_non_ready_state_queues_live_delivery_when_status_is_idle() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                4,
                wardian_core::control::ProviderInputReadiness::Busy,
                None,
            )
            .await;
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
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(delivery[0].runtime_state, "provider_input_not_ready");
        assert_eq!(delivery[0].delivery_state, "queued");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn claude_idle_status_allows_live_delivery_despite_stale_readiness() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "ClaudeOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "claude".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                4,
                wardian_core::control::ProviderInputReadiness::Busy,
                None,
            )
            .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let delivery = deliver_message_to_target(
            None,
            &state,
            "ClaudeOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::LiveOnly,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"queued work".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
        assert_eq!(delivery[0].runtime_state, "live_pty_available");
    }

    #[tokio::test]
    async fn mailbox_drain_can_complete_booting_provider_from_prompt_evidence() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
            agent
                .watch_state
                .lock()
                .unwrap()
                .push_output(b"\r\n\x1b[1m\xe2\x80\xba\x1b[22m Ready");
        }
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                4,
                wardian_core::control::ProviderInputReadiness::Booting,
                None,
            )
            .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let queued = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drained message");

        let expected = expected_terminal_chunks("codex", "queued work");
        assert_eq!(rx.recv().await.unwrap(), expected[0]);
        assert_eq!(rx.recv().await.unwrap(), expected[1]);
        assert_eq!(drained.runtime_state, "mailbox_drain");
        assert_eq!(drained.delivery_state, "submit_sent_unverified");
        assert_eq!(drained.message_id.as_deref(), Some(message_id.as_str()));
        let input_state = state
            .interactions
            .provider_input_state("agent-1")
            .await
            .unwrap();
        assert_eq!(
            input_state.state,
            wardian_core::control::ProviderInputReadiness::Busy
        );
    }

    #[tokio::test]
    async fn mailbox_drain_can_complete_booting_claude_from_idle_status() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "ClaudeOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "claude".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                4,
                wardian_core::control::ProviderInputReadiness::Booting,
                None,
            )
            .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let queued = deliver_message_to_target(
            None,
            &state,
            "ClaudeOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();

        let drained = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1"),
        )
        .await
        .expect("mailbox drain should not hang")
        .unwrap()
        .expect("drained message");

        assert_eq!(drained.runtime_state, "mailbox_drain");
        assert_eq!(drained.delivery_state, "submit_sent_unverified");
        assert_eq!(drained.message_id.as_deref(), Some(message_id.as_str()));
        assert_eq!(rx.try_recv().unwrap(), b"queued work".to_vec());
        assert_eq!(rx.try_recv().unwrap(), b"\r".to_vec());
        let input_state = state
            .interactions
            .provider_input_state("agent-1")
            .await
            .unwrap();
        assert_eq!(
            input_state.state,
            wardian_core::control::ProviderInputReadiness::Busy
        );
    }

    #[test]
    fn claude_ready_prompt_detector_accepts_visible_prompt_tail() {
        assert!(claude_output_has_ready_prompt(
            "ClaudeCode v2.1.150\r\n❯ Try \"write a test\"\r\n────────────────⏵⏵ dontask on · Haiku 4.5"
        ));
    }

    #[tokio::test]
    async fn mailbox_drain_can_complete_booting_gemini_from_prompt_evidence() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "GeminiOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "gemini".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
            agent.watch_state.lock().unwrap().push_output(
                "\r\n? for shortcuts\r\n────────────────────────────────────────────────────────\r\n YOLO Ctrl+Y                                      5 context files · 2 MCP servers · 25 skills\r\n▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\r\n *  Type your message or @path/to/file\r\n▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\r\n workspace (/directory)              /model                      context                quota\r\n".as_bytes(),
            );
        }
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                4,
                wardian_core::control::ProviderInputReadiness::Booting,
                None,
            )
            .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let queued = deliver_message_to_target(
            None,
            &state,
            "GeminiOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drained message");

        assert_eq!(drained.runtime_state, "mailbox_drain");
        assert_eq!(drained.delivery_state, "submit_sent_unverified");
        assert_eq!(drained.message_id.as_deref(), Some(message_id.as_str()));
        assert_eq!(rx.try_recv().unwrap(), b"queued work".to_vec());
        assert_eq!(rx.try_recv().unwrap(), b"\r".to_vec());
        let input_state = state
            .interactions
            .provider_input_state("agent-1")
            .await
            .unwrap();
        assert_eq!(
            input_state.state,
            wardian_core::control::ProviderInputReadiness::Busy
        );
    }

    #[tokio::test]
    async fn mailbox_drain_can_complete_booting_antigravity_from_prompt_evidence() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "AntigravityOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "antigravity".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
            agent.watch_state.lock().unwrap().push_output(
                "\r\n────────────────────────────────────────────────────────\r\n>\r\n────────────────────────────────────────────────────────\r\n  Press up to edit queued messages                                               Gemini 3.5 Flash (High)\r\n".as_bytes(),
            );
        }
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                4,
                wardian_core::control::ProviderInputReadiness::Booting,
                None,
            )
            .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let queued = deliver_message_to_target(
            None,
            &state,
            "AntigravityOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drained message");

        assert_eq!(drained.runtime_state, "mailbox_drain");
        assert_eq!(drained.delivery_state, "submit_sent_unverified");
        assert_eq!(drained.message_id.as_deref(), Some(message_id.as_str()));
        assert_eq!(rx.try_recv().unwrap(), b"queued work".to_vec());
        assert_eq!(rx.try_recv().unwrap(), b"\r".to_vec());
        let input_state = state
            .interactions
            .provider_input_state("agent-1")
            .await
            .unwrap();
        assert_eq!(
            input_state.state,
            wardian_core::control::ProviderInputReadiness::Busy
        );
    }

    #[tokio::test]
    async fn mailbox_drain_marks_provider_busy_after_one_submitted_message() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Processing".to_string();
            agent
                .watch_state
                .lock()
                .unwrap()
                .push_output(b"\r\n\x1b[1m\xe2\x80\xba\x1b[22m Ready");
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let first = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "first queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let second = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "second queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(first[0].delivery_state, "queued");
        assert_eq!(second[0].delivery_state, "queued");
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                4,
                wardian_core::control::ProviderInputReadiness::Ready,
                Some(wardian_core::control::ProviderReadyEvidence::PromptDetected),
            )
            .await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("first message drains");
        let blocked = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap();

        assert_eq!(drained.delivery_state, "submit_sent_unverified");
        assert!(blocked.is_none());
        let expected = expected_terminal_chunks("codex", "first queued work");
        assert_eq!(rx.try_recv().unwrap(), expected[0]);
        assert_eq!(rx.try_recv().unwrap(), expected[1]);
        assert!(rx.try_recv().is_err());
        let input_state = state
            .interactions
            .provider_input_state("agent-1")
            .await
            .unwrap();
        assert_eq!(
            input_state.state,
            wardian_core::control::ProviderInputReadiness::Busy
        );
    }

    #[tokio::test]
    async fn provider_non_ready_state_rejects_approval_action_instead_of_queueing() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                4,
                wardian_core::control::ProviderInputReadiness::Busy,
                None,
            )
            .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let error = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "approve",
            None,
            MessageInputMode::ApprovalAction,
            QueuePolicy::QueueIfBusy,
            Some(&ApprovalAction::Accept),
            None,
        )
        .await
        .unwrap_err();

        assert_eq!(error.code, "request_failed");
        let records = state.mailbox.lock().await.list_for_target("agent-1");
        assert!(records.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn stale_readiness_generation_does_not_drain_mailbox() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                2,
                wardian_core::control::ProviderInputReadiness::Busy,
                None,
            )
            .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let queued = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(queued[0].delivery_state, "queued");
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                1,
                wardian_core::control::ProviderInputReadiness::Ready,
                Some(wardian_core::control::ProviderReadyEvidence::PromptDetected),
            )
            .await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap();

        assert!(drained.is_none());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn mailbox_drain_waits_until_target_is_idle() {
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
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap();

        assert!(drained.is_none());
        assert!(rx.try_recv().is_err());
        let records = state.mailbox.lock().await.list_for_target("agent-1");
        assert_eq!(
            records[0].status,
            crate::state::MailboxMessageStatus::Pending
        );
    }

    #[tokio::test]
    async fn mailbox_drain_missing_sender_leaves_message_pending_for_retry() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Action Required".to_string();
        }

        let queued = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }

        let attempt = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drain attempt");

        assert_eq!(attempt.runtime_state, "mailbox_drain");
        assert_eq!(attempt.delivery_state, "failed");
        assert_eq!(attempt.message_id.as_deref(), Some(message_id.as_str()));
        assert_eq!(
            attempt.error.as_ref().map(|error| error.code.as_str()),
            Some("no_input_channel")
        );
        let records = state.mailbox.lock().await.list_for_target("agent-1");
        assert_eq!(
            records[0].status,
            crate::state::MailboxMessageStatus::Pending
        );
        assert_eq!(records[0].phase, crate::state::MailboxDeliveryPhase::Queued);
    }

    #[tokio::test]
    async fn mailbox_drain_submit_key_failure_marks_failed_without_retry() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Processing".to_string();
            agent
                .watch_state
                .lock()
                .unwrap()
                .push_output(b"\r\n\x1b[1m\xe2\x80\xba\x1b[22m Ready");
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let queued = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }

        let drain = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1");
        tokio::pin!(drain);
        let payload = tokio::select! {
            payload = rx.recv() => payload.expect("payload"),
            attempt = &mut drain => panic!("drain completed before payload was observed: {attempt:?}"),
        };
        let expected = expected_terminal_chunks("codex", "queued work");
        assert_eq!(payload, expected[0]);
        drop(rx);

        let attempt = drain.await.unwrap().expect("drain attempt");

        assert_eq!(attempt.runtime_state, "mailbox_drain");
        assert_eq!(attempt.delivery_state, "failed");
        assert_eq!(attempt.message_id.as_deref(), Some(message_id.as_str()));
        assert_eq!(
            attempt.delivery_phase.as_deref(),
            Some("payload_sent_submit_failed")
        );
        assert_eq!(
            attempt.error.as_ref().map(|error| error.code.as_str()),
            Some("send_failed")
        );
        assert!(attempt
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("partial or unknown"));
        let records = state.mailbox.lock().await.list_for_target("agent-1");
        assert_eq!(
            records[0].status,
            crate::state::MailboxMessageStatus::Failed
        );
        assert_eq!(
            records[0].phase,
            crate::state::MailboxDeliveryPhase::Terminal
        );

        let second_attempt = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap();
        assert!(second_attempt.is_none());
    }

    #[tokio::test]
    async fn mailbox_drain_payload_send_failure_does_not_emit_submit_started() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Processing".to_string();
            agent
                .watch_state
                .lock()
                .unwrap()
                .push_output(b"\r\n\x1b[1m\xe2\x80\xba\x1b[22m Ready");
        }
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        state
            .input_senders
            .write()
            .unwrap()
            .insert("agent-1".to_string(), tx);

        let queued = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "queued work",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }

        let attempt = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drain attempt");

        assert_eq!(attempt.delivery_state, "failed");
        assert_eq!(attempt.message_id.as_deref(), Some(message_id.as_str()));
        assert_eq!(
            attempt.delivery_phase.as_deref(),
            Some("payload_send_failed")
        );
        let records = state.mailbox.lock().await.list_for_target("agent-1");
        assert_eq!(
            records[0].status,
            crate::state::MailboxMessageStatus::Pending
        );
        let agents = state.agents.lock().await;
        let agent = agents.get("agent-1").unwrap();
        let snapshot = agent
            .watch_state
            .lock()
            .unwrap()
            .snapshot_since(None, None)
            .unwrap();
        assert!(!snapshot.events.iter().any(|event| {
            event.kind == "delivery"
                && event.payload["delivery_state"] == "submit_started"
                && event.payload["message_id"] == message_id.as_str()
        }));
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
            QueuePolicy::QueueIfBusy,
            None,
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
            QueuePolicy::QueueIfBusy,
            None,
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
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }

        let error = deliver_message_to_target(
            None,
            &state,
            "agent-1",
            "hello",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
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
        {
            let agents = state.agents.lock().await;
            *agents
                .get("agent-1")
                .unwrap()
                .current_status
                .lock()
                .unwrap() = "Idle".to_string();
            *agents
                .get("agent-2")
                .unwrap()
                .current_status
                .lock()
                .unwrap() = "Idle".to_string();
        }
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
            QueuePolicy::QueueIfBusy,
            None,
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
            QueuePolicy::QueueIfBusy,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(rx.try_recv().is_err());
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

    #[test]
    fn ask_response_preserves_reply_when_watch_evidence_fails() {
        let reply = wardian_core::control::StructuredReply {
            request_id: "ask_testrequest03".to_string(),
            status: wardian_core::control::ReplyStatus::Done,
            body: "finished despite watch gap".to_string(),
            target_session_id: "agent-1".to_string(),
            source_session_id: Some("agent-1".to_string()),
            replied_at: "2026-05-22T00:00:00.000Z".to_string(),
        };
        let response = build_ask_response_with_watch_result(
            "ask_testrequest03".to_string(),
            "CoderOne".to_string(),
            Vec::new(),
            reply,
            WatchAgentSnapshot {
                uuid: "agent-1".to_string(),
                name: "CoderOne".to_string(),
                provider: "codex".to_string(),
                status: "idle".to_string(),
                last_status_at: None,
            },
            Err(ControlError::coded(
                "cursor_expired",
                "watch evidence cursor expired",
            )),
        );

        assert!(response.ok);
        assert_eq!(response.reply.body, "finished despite watch gap");
        assert_eq!(
            response
                .watch_error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("cursor_expired")
        );
        assert_eq!(response.watch.agent.uuid, "agent-1");
        assert_eq!(response.watch.output.text, "");
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

    fn snapshot_with_output(cursor: &str, text: &str) -> crate::state::agent_watch::WatchSnapshot {
        crate::state::agent_watch::WatchSnapshot {
            cursor: cursor.to_string(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: cursor.to_string(),
                text: text.to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
            raw_output: wardian_core::control::WatchOutput {
                cursor: cursor.to_string(),
                text: text.to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
            transcript: wardian_core::control::WatchTranscript {
                cursor: cursor.to_string(),
                messages: Vec::new(),
                latest_text: String::new(),
                truncated: false,
                omitted_bytes: 0,
            },
        }
    }

    fn test_watch_agent() -> WatchAgentSnapshot {
        WatchAgentSnapshot {
            uuid: "agent-1".to_string(),
            name: "CoderOne".to_string(),
            provider: "mock".to_string(),
            status: "idle".to_string(),
            last_status_at: None,
        }
    }

    #[test]
    fn watch_response_default_includes_readable_output_without_raw_output() {
        let mut state = crate::state::AgentWatchState::new("agent-1".to_string(), 16, 1024);
        state.push_output("\u{1b}[31mreadable\u{1b}[0m".as_bytes());
        let snapshot = state.snapshot_since(None, Some(1024)).unwrap();
        let response = build_agent_watch_response(
            test_watch_agent(),
            snapshot,
            &WatchIncludes::from_values(&[]),
        );

        assert_eq!(response.output.text, "readable");
        assert!(response.raw_output.is_none());
        assert!(response.transcript.is_some());
    }

    #[test]
    fn watch_response_raw_include_preserves_raw_terminal_text() {
        let mut state = crate::state::AgentWatchState::new("agent-1".to_string(), 16, 1024);
        state.push_output("\u{1b}[31mreadable\u{1b}[0m".as_bytes());
        let snapshot = state.snapshot_since(None, Some(1024)).unwrap();
        let response = build_agent_watch_response(
            test_watch_agent(),
            snapshot,
            &WatchIncludes::from_values(&["raw_output".to_string(), "output".to_string()]),
        );

        assert_eq!(response.output.text, "readable");
        assert_eq!(
            response.raw_output.as_ref().unwrap().text,
            "\u{1b}[31mreadable\u{1b}[0m"
        );
    }

    #[test]
    fn output_condition_matches_transcript_clean_output_and_raw_fallback() {
        let mut transcript_snapshot = snapshot_with_output("agent-1:1", "");
        transcript_snapshot.transcript.latest_text = "Final REVIEW_DONE".to_string();
        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("REVIEW_DONE".to_string()),
            &transcript_snapshot,
            None,
        ));

        let clean_snapshot = snapshot_with_output("agent-1:2", "Final REVIEW_DONE");
        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("REVIEW_DONE".to_string()),
            &clean_snapshot,
            None,
        ));

        let mut raw_snapshot = snapshot_with_output("agent-1:3", "");
        raw_snapshot.raw_output.text = "Final \u{1b}[31mREVIEW_DONE\u{1b}[0m".to_string();
        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("REVIEW_DONE".to_string()),
            &raw_snapshot,
            None,
        ));
    }

    #[test]
    fn output_condition_checks_later_surfaces_after_echo_match() {
        let mut snapshot = snapshot_with_output(
            "agent-1:4",
            "\u{1b}[1m›\u{1b}[22m Say REVIEW_DONE when finished\r\nActual response: REVIEW_DONE",
        );
        snapshot.transcript.latest_text = "Say REVIEW_DONE when finished".to_string();

        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("REVIEW_DONE".to_string()),
            &snapshot,
            Some("Say REVIEW_DONE when finished"),
        ));
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
        let snapshot = snapshot_with_output(
            "agent-1:0000000000000001",
            "\u{1b}[1m›\u{1b}[22m From Wardian-Arch: Say AUTO_TEST_2_DONE when finished\r\n  gpt-5.5 high · D:\\Development\\Wardian",
        );

        assert!(!watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("Say AUTO_TEST_2_DONE when finished"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_matches_provider_response_after_echo() {
        let snapshot = snapshot_with_output(
            "agent-1:0000000000000002",
            "\u{1b}[1m›\u{1b}[22m Say AUTO_TEST_2_DONE when finished\r\nActual response: AUTO_TEST_2_DONE",
        );

        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("Say AUTO_TEST_2_DONE when finished"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_ignores_codex_repaint_prompt_fragment() {
        let snapshot = snapshot_with_output(
            "agent-1:0000000000000003",
            "\u{1b}[2J\u{1b}[H\u{1b}[1m›\u{1b}[22m From Wardian-Arch: Capture the README demo GIF\r\n  and end exactly with DEMO_GIF_DONE  gpt-5.5 high · D:\\Development\\Wardian · 75% context left",
        );

        assert!(!watch_condition_matches(
            &WatchCondition::OutputContains("DEMO_GIF_DONE".to_string()),
            &snapshot,
            Some("Capture the README demo GIF and end exactly with DEMO_GIF_DONE"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_matches_response_after_codex_repaint_echo() {
        let snapshot = snapshot_with_output(
            "agent-1:0000000000000004",
            "\u{1b}[2J\u{1b}[H\u{1b}[1m›\u{1b}[22m From Wardian-Arch: Capture the README demo GIF\r\n  and end exactly with DEMO_GIF_DONE  gpt-5.5 high · D:\\Development\\Wardian · 75% context left\r\nFinal response: DEMO_GIF_DONE",
        );

        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("DEMO_GIF_DONE".to_string()),
            &snapshot,
            Some("Capture the README demo GIF and end exactly with DEMO_GIF_DONE"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_matches_exact_marker_response_after_echo() {
        let snapshot = snapshot_with_output(
            "agent-1:0000000000000005",
            "\u{1b}[1m›\u{1b}[22m Say AUTO_TEST_2_DONE when finished\r\n  AUTO_TEST_2_DONE",
        );

        assert!(watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("Say AUTO_TEST_2_DONE when finished"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_ignores_origin_prefixed_json_echo() {
        let snapshot = snapshot_with_output(
            "agent-1:0000000000000006",
            "From Wardian agent agent-1: AUTO_TEST_2_DONE\r\n{\"type\":\"model\",\"content\":\"From Wardian agent agent-1: AUTO_TEST_2_DONE\"}",
        );

        assert!(!watch_condition_matches(
            &WatchCondition::OutputContains("AUTO_TEST_2_DONE".to_string()),
            &snapshot,
            Some("AUTO_TEST_2_DONE"),
        ));
    }

    #[test]
    fn output_condition_with_ask_echo_guard_matches_origin_prefixed_response_after_echo() {
        let snapshot = snapshot_with_output(
            "agent-1:0000000000000007",
            "From Wardian agent agent-1: AUTO_TEST_2_DONE\r\n{\"type\":\"model\",\"content\":\"From Wardian agent agent-1: AUTO_TEST_2_DONE\"}\r\nActual response after echo: From Wardian agent agent-1: AUTO_TEST_2_DONE",
        );

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
