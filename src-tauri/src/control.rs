use crate::manager;
use crate::state::conversation_archive::{
    effective_conversation_logging, ConversationArchiveContext,
};
use crate::state::{AppState, MailboxMessageDraft, MailboxMessageRecord};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{
    AgentDoctorResponse, AgentListResponse, AgentResponse, AgentUpdateResponse, AgentWatchResponse,
    AgentWorktreeListResponse, AgentWorktreeMutationResponse, AgentWorktreeSummary, ApprovalAction,
    AskManyResponse, AskResponse, AskTargetOutcome, AskTargetResponse, CodexPluginDiagnostic,
    ControlRequest, ConversationListResponse, ConversationShowResponse, DeliveryDetail,
    DeliveryErrorDetail, DeliveryTransportKind, InboxNotificationKind, InboxNotificationPayload,
    InboxNotificationResponse, InteractionBodyRef, InteractionStatus, MessageInputMode,
    MessageOrigin, OkResponse, ProviderInputReadiness, ProviderReadyEvidence, QueuePolicy,
    ReplyResponse, ReplyStatus, SendMessageResponse, StructuredReply, WatchAgentSnapshot,
    WatchDeliverySnapshot, WatchEvidenceError,
};
use wardian_core::conversations::ConversationLoggingSetting;
use wardian_core::identity::{normalize_status, AgentIdentity, StatusSource};
use wardian_core::models::{AgentChatEvent, AgentChatEventKind, AgentChatRole};

const STRUCTURED_ASK_INLINE_MESSAGE_MAX_BYTES: usize = 4096;
const STRUCTURED_ASK_REQUESTS_DIR: &str = "requests";
const PROVIDER_TURN_START_TIMEOUT_MS: u64 = 10_000;
const MAX_HEADLESS_DELIVERY_TIMEOUT: Duration = Duration::from_secs(15 * 60);

async fn rollback_agent_update(
    state: &AppState,
    session_id: &str,
    previous_config: wardian_core::models::AgentConfig,
) -> Result<(), String> {
    let snapshot =
        crate::commands::agent::restore_agent_config_in_state(state, session_id, previous_config)
            .await?;
    manager::try_save_state_snapshot(&snapshot)
}

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

        ControlRequest::AgentRestart { target } => {
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            crate::commands::agent::resume_agent(uuid, app.state::<AppState>(), app.clone())
                .await
                .map_err(ControlError::request_failed)?;
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

        ControlRequest::AgentModels {
            provider,
            force_refresh,
        } => ok_json(&crate::providers::models::model_catalog(&provider, force_refresh).await),

        ControlRequest::AgentSpawn {
            provider,
            class,
            name,
            workspace,
            model,
            reasoning_effort,
        } => {
            use crate::commands::agent::spawn_agent;
            let req = build_spawn_agent_request(
                provider,
                class,
                name,
                workspace,
                model,
                reasoning_effort,
            )
            .map_err(ControlError::bad_request)?;
            let config = spawn_agent(req, app.state::<AppState>(), app.clone())
                .await
                .map_err(ControlError::request_failed)?;
            let identity = agent_config_to_identity(&config, app).await;
            ok_json(&AgentResponse::new(identity))
        }

        ControlRequest::AgentUpdate {
            target,
            class,
            workspace,
            description,
            model,
            reasoning_effort,
        } => {
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            let home = crate::utils::fs::get_wardian_home()
                .ok_or_else(|| ControlError::request_failed("Could not locate Wardian home"))?;
            let classes = wardian_core::classes::initialize_classes(&home)
                .map_err(ControlError::request_failed)?;
            if let Some(class) = class.as_deref() {
                if !classes
                    .iter()
                    .any(|definition| definition.name.eq_ignore_ascii_case(class.trim()))
                {
                    return Err(ControlError::not_found(format!(
                        "agent class not found: {class}"
                    )));
                }
            }

            let state = app.state::<AppState>();
            let outcome = crate::commands::agent::update_agent_fields_in_state(
                state.inner(),
                &uuid,
                crate::commands::agent::AgentUpdateFields {
                    class: class.as_deref(),
                    workspace: workspace.as_deref(),
                    description: description.as_deref(),
                    model: model.as_deref(),
                    reasoning_effort: reasoning_effort.as_deref(),
                },
                &classes,
            )
            .await
            .map_err(ControlError::bad_request)?;
            if let Err(error) = manager::try_save_state_snapshot(&outcome.state_snapshot) {
                let rollback_error =
                    rollback_agent_update(state.inner(), &uuid, outcome.previous_config.clone())
                        .await
                        .err()
                        .map(|rollback| format!("; rollback also failed: {rollback}"))
                        .unwrap_or_default();
                return Err(ControlError::request_failed(format!(
                    "Failed to persist agent update: {error}{rollback_error}"
                )));
            }
            let workspace =
                crate::utils::fs::resolve_cwd(&outcome.config.folder, &outcome.config.session_id)
                    .to_string_lossy()
                    .to_string();
            let project = wardian_core::db::project_name_from_workspace(&workspace);
            let metadata_error = wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
                session_id: &outcome.config.session_id,
                session_name: &outcome.config.session_name,
                description: &outcome.config.description,
                agent_class: &outcome.config.agent_class,
                provider: &outcome.config.provider,
                workspace: Some(&workspace),
                project: project.as_deref(),
                is_off: outcome.config.is_off,
                created_at: None,
            })
            .err()
            .map(|error| error.to_string());
            if let Some(error) = metadata_error {
                let rollback_error =
                    rollback_agent_update(state.inner(), &uuid, outcome.previous_config.clone())
                        .await
                        .err()
                        .map(|rollback| format!("; rollback also failed: {rollback}"))
                        .unwrap_or_default();
                return Err(ControlError::request_failed(format!(
                    "Failed to persist agent metadata: {error}{rollback_error}"
                )));
            }
            let _ = app.emit("agents-updated", ());
            let restart_required =
                agent_update_requires_restart(&outcome.updated_fields, outcome.config.is_off);
            let identity = agent_config_to_identity(&outcome.config, app).await;
            ok_json(&AgentUpdateResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                ok: true,
                agent: identity,
                updated_fields: outcome.updated_fields,
                restart_required,
            })
        }

        ControlRequest::AgentDoctor { target } => {
            if target == "all" || target.starts_with("class:") {
                return Err(ControlError::not_supported(
                    "agent doctor requires a single agent name or uuid",
                ));
            }
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            ok_json(&build_agent_doctor_response(app, &uuid).await?)
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

        ControlRequest::ConversationList { agent, scope_all } => {
            let state = app.state::<AppState>();
            let response: ConversationListResponse =
                crate::commands::conversation::list_conversations_for_state(
                    &state,
                    agent.as_deref(),
                    scope_all,
                )
                .map_err(ControlError::request_failed)?;
            ok_json(&response)
        }

        ControlRequest::ConversationShow { conversation_id } => {
            let state = app.state::<AppState>();
            let response: ConversationShowResponse =
                crate::commands::conversation::show_conversation_for_state(
                    &state,
                    &conversation_id,
                )
                .map_err(ControlError::request_failed)?;
            ok_json(&response)
        }

        ControlRequest::ArtifactPresent {
            path,
            title,
            description,
            artifact_id,
            force_new,
            addressed_comment_ids,
            origin,
        } => {
            let MessageOrigin::WardianAgent { session_id } = origin;
            let state = app.state::<AppState>();
            let config = {
                let agents = state.agents.lock().await;
                agents
                    .get(&session_id)
                    .map(|agent| agent.config.clone())
                    .ok_or_else(|| {
                        ControlError::coded(
                            "invalid_origin",
                            "artifact origin is not a live Wardian agent session",
                        )
                    })?
            };
            let config = config
                .lock()
                .map_err(|_| {
                    ControlError::request_failed("agent configuration lock is unavailable")
                })?
                .clone();
            let store = artifact_store()?;
            let emit_app = app.clone();
            let service = crate::artifact_service::ArtifactService::new(
                store,
                state.artifact_runtime.clone(),
                move |event| {
                    emit_app
                        .emit(crate::artifact_service::ARTIFACT_PRESENTED_EVENT, event)
                        .map_err(|error| error.to_string())
                },
            );
            let response = service
                .present(
                    config,
                    crate::artifact_service::ArtifactPresentationRequestV1 {
                        origin_session_id: session_id,
                        path,
                        title,
                        description,
                        artifact_id,
                        force_new,
                        addressed_comment_ids,
                    },
                )
                .await
                .map_err(artifact_service_control_error)?;
            ok_json(&response)
        }

        ControlRequest::ArtifactShow {
            artifact_id,
            version_id,
        } => {
            let state = app.state::<AppState>();
            let service = crate::artifact_service::ArtifactService::new(
                artifact_store()?,
                state.artifact_runtime.clone(),
                |_| Ok(()),
            );
            let response = service
                .show(artifact_id, version_id)
                .await
                .map_err(artifact_service_control_error)?;
            ok_json(&response)
        }

        ControlRequest::ArtifactReviewShow { .. } => Err(ControlError::coded(
            "review_not_found",
            "artifact reviews are not available for this thread",
        )),

        ControlRequest::WatchlistsChanged => {
            let _ = app.emit("watchlists-updated", ());
            ok_json(&OkResponse::new())
        }

        request @ ControlRequest::WorkflowRun { .. } => {
            handle_workflow_run_control(app, workflow_run_control_launch(request)?).await
        }

        ControlRequest::SendMessage {
            target,
            message,
            thread,
            input_mode,
            queue_policy,
            approval_action,
            origin,
            target_scope,
            headless_timeout_ms,
        } => {
            let state = app.state::<AppState>();
            let scope_all = target_scope.as_deref() == Some("all");
            let delivery = deliver_message_to_target_with_headless_timeout(
                Some(app),
                &state,
                &target,
                &message,
                thread.as_deref(),
                input_mode,
                queue_policy,
                approval_action.as_ref(),
                origin.as_ref(),
                scope_all,
                bounded_headless_delivery_timeout(headless_timeout_ms),
            )
            .await?;
            record_conversation_delivery(&state, &delivery, &message, origin.as_ref()).await;
            ok_json(&SendMessageResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                ok: true,
                delivery,
            })
        }

        ControlRequest::NotifyCreate {
            notification,
            origin,
        } => {
            let MessageOrigin::WardianAgent { session_id } = origin;
            validate_inbox_notification(&notification)?;
            let state = app.state::<AppState>();
            {
                let agents = state.agents.lock().await;
                if !agents.contains_key(&session_id) {
                    return Err(ControlError::coded(
                        "invalid_origin",
                        "notification origin is not a live Wardian agent session",
                    ));
                }
            }
            let record = state
                .interactions
                .create_notification_durable(session_id, notification)
                .await
                .map_err(notification_control_error)?;
            let _ = app.emit("inbox-updated", ());
            ok_json(&InboxNotificationResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                notification_id: record.id,
                status: record.status,
                decision: None,
            })
        }

        ControlRequest::NotifyWait {
            notification_id,
            timeout_ms,
            origin,
        } => {
            let MessageOrigin::WardianAgent { session_id } = origin;
            let state = app.state::<AppState>();
            let timeout = Duration::from_millis(timeout_ms.unwrap_or(30 * 60 * 1000));
            let started = std::time::Instant::now();
            loop {
                let record = state
                    .interactions
                    .expire_notification_if_needed(&notification_id)
                    .await
                    .ok_or_else(|| {
                        ControlError::coded("not_found", "notification was not found")
                    })?;
                if record.sender_session_id.as_deref() != Some(session_id.as_str()) {
                    return Err(ControlError::coded(
                        "unauthorized",
                        "notification does not belong to this agent session",
                    ));
                }
                match record.status {
                    InteractionStatus::Completed | InteractionStatus::Expired => {
                        let decision = state
                            .interactions
                            .notification_decision(&notification_id)
                            .await;
                        let _ = app.emit("inbox-updated", ());
                        return ok_json(&InboxNotificationResponse {
                            schema: wardian_core::control::CONTROL_SCHEMA,
                            notification_id,
                            status: record.status,
                            decision,
                        });
                    }
                    _ if started.elapsed() >= timeout => {
                        return Err(ControlError::coded(
                            "notify_timeout",
                            "notification was not resolved before the requested timeout",
                        ));
                    }
                    _ => tokio::time::sleep(Duration::from_millis(150)).await,
                }
            }
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

        ControlRequest::AskMany {
            targets,
            message,
            thread,
            tail_bytes,
            timeout_ms,
            origin,
        } => {
            handle_structured_ask_many(
                app,
                &targets,
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
            let reply = submit_structured_reply(
                &state,
                &request_id,
                status,
                &body,
                origin.as_ref(),
                Some(app),
            )
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
    model: Option<String>,
    reasoning_effort: Option<String>,
) -> Result<crate::commands::agent::SpawnAgentRequest, String> {
    let mut config_override = wardian_core::models::AgentConfig {
        provider,
        ..Default::default()
    };
    config_override.reset_provider_config_for_provider();
    crate::commands::agent::apply_agent_model_selection_update(
        &mut config_override,
        model.as_deref(),
        reasoning_effort.as_deref(),
    )?;
    Ok(crate::commands::agent::SpawnAgentRequest {
        session_name: name.unwrap_or_default(),
        agent_class: class,
        folder: workspace.unwrap_or_default(),
        resume_session: None,
        is_off: None,
        config_override: Some(config_override),
    })
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

#[derive(Debug)]
struct WorkflowRunControlLaunch {
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<serde_json::Value>,
    bindings: Option<std::collections::HashMap<String, String>>,
    assignments: Option<wardian_core::models::WorkflowAssignments>,
}

fn workflow_run_control_launch(
    request: ControlRequest,
) -> Result<WorkflowRunControlLaunch, ControlError> {
    match request {
        ControlRequest::WorkflowRun {
            path,
            provider,
            workspace,
            input,
            bindings,
            assignments,
        } => Ok(WorkflowRunControlLaunch {
            path,
            provider,
            workspace,
            input,
            bindings,
            assignments,
        }),
        _ => Err(ControlError::bad_request(
            "expected workflow_run control request",
        )),
    }
}

async fn handle_workflow_run_control(
    app: &AppHandle,
    launch: WorkflowRunControlLaunch,
) -> Result<String, ControlError> {
    let result = crate::commands::workflow::workflow_run_from_control(
        app.state::<AppState>(),
        app.clone(),
        launch.path,
        launch.provider,
        launch.workspace,
        launch.input,
        launch.bindings,
        launch.assignments,
    )
    .await
    .map_err(ControlError::request_failed)?;
    ok_json(&result)
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

async fn build_agent_doctor_response(
    app: &AppHandle,
    session_id: &str,
) -> Result<AgentDoctorResponse, ControlError> {
    let state = app.state::<AppState>();
    let config = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(session_id)
            .ok_or_else(|| ControlError::not_found(format!("agent not found: {session_id}")))?;
        let config = agent
            .config
            .lock()
            .map_err(|_| ControlError::request_failed("agent config lock poisoned"))?
            .clone();
        config
    };
    let agent = agent_config_to_identity(&config, app).await;
    if config.provider != "codex" {
        return Ok(AgentDoctorResponse {
            schema: wardian_core::control::CONTROL_SCHEMA,
            agent,
            applicable: false,
            codex_home: None,
            plugins: Vec::new(),
            plugin_inspection_error: None,
            launch_flags: Vec::new(),
            restart_required: false,
            reasons: vec!["not_applicable".to_string()],
        });
    }

    let wardian_home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| ControlError::request_failed("Could not locate Wardian home"))?;
    let codex_home = crate::utils::fs::habitat_codex_home(
        &wardian_home
            .join("agents")
            .join(&config.session_id)
            .join("habitat"),
    );
    let provider = crate::providers::ProviderFactory::resolve("codex")
        .map_err(ControlError::request_failed)?;
    let launch_flags = provider.get_spawn_args(&config, false);

    let mut reasons = Vec::new();
    let (plugins, plugin_inspection_error) =
        match crate::utils::fs::inspect_codex_plugins(&codex_home) {
            Ok(statuses) => (
                statuses
                    .into_iter()
                    .map(|status| CodexPluginDiagnostic {
                        selector: status.selector,
                        installed: status.installed,
                        enabled: status.enabled,
                    })
                    .collect(),
                None,
            ),
            Err(error) => {
                reasons.push("plugin_inspection_failed".to_string());
                (Vec::new(), Some(error))
            }
        };
    reasons.sort();
    reasons.dedup();

    Ok(AgentDoctorResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        agent,
        applicable: true,
        codex_home: Some(codex_home.to_string_lossy().to_string()),
        plugins,
        plugin_inspection_error,
        launch_flags,
        restart_required: false,
        reasons,
    })
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

/// Neighbors-scoped broadcast/class/name resolution when the sender is an agent.
/// UUID and exact-name misses fall back to global (soft boundary: explicit
/// targeting always works). scope_all=true disables scoping (e.g. orchestrator broadcast).
async fn resolve_send_targets_scoped(
    state: &AppState,
    target: &str,
    sender_session_id: Option<&str>,
    scope_all: bool,
) -> Vec<String> {
    let global = resolve_send_targets_in_state(state, target).await;
    let Some(sender) = sender_session_id.filter(|_| !scope_all) else {
        return global;
    };

    // Exact UUID targeting is never scoped.
    if global.len() == 1 && global[0] == target {
        return global;
    }

    let Some(home) = crate::utils::fs::get_wardian_home() else {
        crate::utils::logging::log_debug(
            "[Wardian] wardian home unavailable; send target resolution falling back to global scope",
        );
        return global;
    };
    let topology = wardian_core::topology::load_topology(&home);
    let refs = state.topology_agent_refs().await;

    let neighbors = wardian_core::topology::resolve_neighbors(sender, &topology, &refs);
    let allowed = neighbors.member_uuids();

    if target == "all" || target.starts_with("class:") {
        return global
            .into_iter()
            .filter(|id| allowed.contains(id))
            .collect();
    }

    // Bare name: prefer neighbors match; fall back to global exact match.
    let neighbors_matches: Vec<String> = global
        .iter()
        .filter(|id| allowed.contains(*id))
        .cloned()
        .collect();
    if neighbors_matches.is_empty() {
        global
    } else {
        neighbors_matches
    }
}

#[cfg(test)]
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
    scope_all: bool,
) -> Result<Vec<DeliveryDetail>, ControlError> {
    deliver_message_to_target_with_headless_timeout(
        app,
        state,
        target,
        message,
        thread,
        input_mode,
        queue_policy,
        approval_action,
        origin,
        scope_all,
        crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn deliver_message_to_target_with_headless_timeout(
    app: Option<&AppHandle>,
    state: &AppState,
    target: &str,
    message: &str,
    thread: Option<&str>,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    approval_action: Option<&ApprovalAction>,
    origin: Option<&MessageOrigin>,
    scope_all: bool,
    headless_timeout: Duration,
) -> Result<Vec<DeliveryDetail>, ControlError> {
    validate_send_message_options(target, thread, input_mode)?;
    let sender_session_id = origin
        .as_ref()
        .map(|MessageOrigin::WardianAgent { session_id }| session_id.as_str());
    let session_ids =
        resolve_send_targets_scoped(state, target, sender_session_id, scope_all).await;
    if session_ids.is_empty() {
        return Err(ControlError::not_found(format!(
            "no agents matched target: {target}"
        )));
    }

    let target_infos = delivery_target_infos(state, &session_ids).await?;
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
        let sender_session_id =
            origin.map(|MessageOrigin::WardianAgent { session_id }| session_id.clone());
        let interaction = state
            .interactions
            .create_message_durable(
                sender_session_id,
                vec![info.uuid.clone()],
                InteractionBodyRef::Inline {
                    body: outbound_message.clone(),
                },
            )
            .await
            .map_err(ControlError::request_failed)?;
        let interaction_id = interaction.id.clone();
        if let Some(app) = app {
            let _ = app.emit("pair-activity-changed", ());
        }
        let route = if input_mode == MessageInputMode::ApprovalAction
            || matches!(queue_policy, QueuePolicy::MailboxOnly)
        {
            decide_delivery_route(&info.status, input_mode, queue_policy, approval_action)
        } else if !status_uses_headless_delivery(&info.status)
            && provider_input_has_known_not_ready_state(state, &info.uuid).await
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
        } else if active_conversation_lease_for_delivery(&info) {
            match queue_policy {
                QueuePolicy::QueueIfBusy => DeliveryRoute::Mailbox {
                    runtime_state: "conversation_leased",
                },
                QueuePolicy::LiveOnly => DeliveryRoute::Reject {
                    failure: "conversation_leased",
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
                    interaction_id.clone(),
                    info,
                    outbound_message,
                    input_mode,
                    queue_policy,
                    approval_action,
                    origin,
                    runtime_state,
                )
                .await?;
                persist_interaction_delivery_attempt(
                    state,
                    &interaction_id,
                    &detail.uuid,
                    DeliveryTransportKind::LiveSurface,
                    &detail,
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
                persist_interaction_delivery_attempt(
                    state,
                    &interaction_id,
                    &detail.uuid,
                    DeliveryTransportKind::LiveSurface,
                    &detail,
                )
                .await;
                record_delivery_attempt(state, &detail).await;
                delivery.push(detail);
            }
            DeliveryRoute::Live => {
                let target_uuid = info.uuid.clone();
                let result = crate::delivery::submit_live_surface_prompt(
                    app,
                    state,
                    crate::delivery::LiveSurfacePromptRequest {
                        session_id: target_uuid.clone(),
                        prompt: outbound_message,
                        interaction_id: Some(interaction_id.clone()),
                        input_mode,
                        queue_policy,
                        approval_action: approval_action.cloned(),
                        origin: origin.cloned(),
                        runtime_state: "live_pty_available",
                        mark_prompt_started: true,
                        payload_sent_detail: None,
                        delivery_message_id: None,
                    },
                )
                .await;
                match result {
                    Ok(result) => {
                        delivered += 1;
                        delivery.push(result.detail);
                    }
                    Err(error) => {
                        let error_message = error.to_string();
                        failures.push(format!("{}: {error_message}", info.uuid));
                        if let Some(detail) = error.detail {
                            delivery.push(detail);
                        } else {
                            let mut detail = failed_delivery_detail(
                                info,
                                "live_pty_available",
                                "send_failed",
                                error_message,
                                input_mode,
                                queue_policy,
                            );
                            detail.message_id = Some(interaction_id.clone());
                            persist_interaction_delivery_attempt(
                                state,
                                &interaction_id,
                                &target_uuid,
                                DeliveryTransportKind::LiveSurface,
                                &detail,
                            )
                            .await;
                            record_delivery_attempt(state, &detail).await;
                            delivery.push(detail);
                        }
                    }
                }
            }
            DeliveryRoute::Headless => {
                match deliver_headless_message(
                    state,
                    HeadlessMessageDeliveryRequest {
                        app,
                        info: &info,
                        interaction_id: &interaction_id,
                        prompt: &outbound_message,
                        input_mode,
                        queue_policy,
                        origin,
                        timeout: headless_timeout,
                    },
                )
                .await
                {
                    HeadlessMessageDelivery::Completed(detail) => {
                        if detail.delivery_state == "provider_applied" {
                            delivered += 1;
                        } else {
                            failures.push(format!(
                                "{}: {}",
                                detail.uuid,
                                detail
                                    .error
                                    .as_ref()
                                    .map(|error| error.message.as_str())
                                    .unwrap_or("headless delivery failed")
                            ));
                        }
                        delivery.push(*detail);
                    }
                    HeadlessMessageDelivery::Busy(current_info) => {
                        // The preflight lease check is intentionally only an
                        // optimization. A competing sender or lifecycle
                        // operation can claim the agent after it; QueueIfBusy
                        // must still queue rather than start a second provider
                        // process against the same conversation.
                        queued += 1;
                        let current_info = *current_info;
                        let queued_uuid = current_info.uuid.clone();
                        let queued_status = current_info.status.clone();
                        let detail = enqueue_mailbox_delivery(
                            state,
                            interaction_id.clone(),
                            current_info,
                            outbound_message,
                            input_mode,
                            queue_policy,
                            approval_action,
                            origin,
                            "conversation_leased",
                        )
                        .await?;
                        persist_interaction_delivery_attempt(
                            state,
                            &interaction_id,
                            &detail.uuid,
                            DeliveryTransportKind::LiveSurface,
                            &detail,
                        )
                        .await;
                        record_delivery_attempt(state, &detail).await;
                        if let Some(app) = app {
                            spawn_mailbox_drain_if_idle(app, &queued_uuid, &queued_status);
                        }
                        delivery.push(detail);
                    }
                }
            }
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
    Headless,
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
            if matches!(queue_policy, QueuePolicy::QueueIfBusy)
                && input_mode == MessageInputMode::Message
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
            QueuePolicy::QueueIfBusy if input_mode == MessageInputMode::Message => {
                DeliveryRoute::Headless
            }
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

fn status_uses_headless_delivery(status: &str) -> bool {
    matches!(status, "off" | "error")
}

fn bounded_headless_delivery_timeout(timeout_ms: Option<u64>) -> Duration {
    timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT)
        .max(Duration::from_secs(1))
        .min(MAX_HEADLESS_DELIVERY_TIMEOUT)
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

pub(crate) async fn submit_approval_action_via_sender<S>(
    tx: &S,
    provider: &str,
    action: &ApprovalAction,
) -> Result<
    crate::utils::delivery_transaction::TerminalDeliveryOutcome,
    crate::utils::delivery_transaction::TerminalDeliveryError,
>
where
    S: crate::utils::delivery_transaction::TerminalInputSink + ?Sized,
{
    let bytes = approval_action_bytes(provider, action);
    tx.send_bytes(bytes).await.map_err(|error| {
        crate::utils::delivery_transaction::TerminalDeliveryError::terminal_state_unknown(
            "approval_send_failed",
            format!("Failed to send approval action: {error}"),
        )
    })?;
    Ok(
        crate::utils::delivery_transaction::TerminalDeliveryOutcome {
            delivery_state: "approval_submitted".to_string(),
            delivery_phase: "approval_key_sent".to_string(),
            observed_state: Some("bytes_sent".to_string()),
            reason: None,
        },
    )
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_mailbox_delivery(
    state: &AppState,
    interaction_id: String,
    info: DeliveryTargetInfo,
    body: String,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    approval_action: Option<&ApprovalAction>,
    origin: Option<&MessageOrigin>,
    runtime_state: &str,
) -> Result<DeliveryDetail, ControlError> {
    let record = {
        let mut mailbox = state.mailbox.lock().await;
        let record = mailbox.enqueue(MailboxMessageDraft {
            interaction_id,
            target_session_id: info.uuid.clone(),
            body,
            input_mode,
            queue_policy,
            approval_action: approval_action.cloned(),
            origin: origin.cloned(),
        });
        if let Err(error) = wardian_core::db::upsert_mailbox_message(&record) {
            mailbox.remove(&record.id);
            return Err(ControlError::request_failed(format!(
                "failed to persist queued mailbox message: {error}"
            )));
        }
        record
    };

    Ok(DeliveryDetail {
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
    })
}

enum HeadlessMessageDelivery {
    Completed(Box<DeliveryDetail>),
    Busy(Box<DeliveryTargetInfo>),
}

struct HeadlessMessageDeliveryRequest<'a> {
    app: Option<&'a AppHandle>,
    info: &'a DeliveryTargetInfo,
    interaction_id: &'a str,
    prompt: &'a str,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    origin: Option<&'a MessageOrigin>,
    timeout: Duration,
}

#[derive(Debug)]
enum HeadlessMessageLeaseError {
    Busy,
    Failed(String),
}

async fn deliver_headless_message(
    state: &AppState,
    request: HeadlessMessageDeliveryRequest<'_>,
) -> HeadlessMessageDelivery {
    let HeadlessMessageDeliveryRequest {
        app,
        info,
        interaction_id,
        prompt,
        input_mode,
        queue_policy,
        origin,
        timeout,
    } = request;
    // Direct offline delivery runs a provider against the target agent's
    // workspace. Hold the same home-wide shared guard as workflow drives
    // before taking a conversation lease, so a managed-worktree deletion
    // cannot remove that workspace before or during provider execution.
    let _headless_execution =
        match wardian_core::workflow_execution_lock::acquire_headless_execution_guard() {
            Ok(guard) => guard,
            Err(error) => {
                let detail = headless_message_failure_detail(
                    info,
                    interaction_id,
                    input_mode,
                    queue_policy,
                    "headless_execution_blocked",
                    error,
                );
                persist_interaction_delivery_attempt(
                    state,
                    interaction_id,
                    &info.uuid,
                    DeliveryTransportKind::HeadlessProcess,
                    &detail,
                )
                .await;
                record_delivery_attempt(state, &detail).await;
                return HeadlessMessageDelivery::Completed(Box::new(detail));
            }
        };
    // Every headless path claims the persisted lease before the in-process
    // lifecycle gate. Workflows and lifecycle mutations use the same order, so
    // a local waiter never holds the gate while another Wardian process holds
    // the lease it needs to finish.
    let lease = match acquire_headless_message_lease(info, interaction_id) {
        Ok(lease) => lease,
        Err(HeadlessMessageLeaseError::Busy) => {
            return HeadlessMessageDelivery::Busy(Box::new(
                delivery_target_info(state, &info.uuid)
                    .await
                    .unwrap_or_else(|_| info.clone()),
            ))
        }
        Err(HeadlessMessageLeaseError::Failed(error)) => {
            let detail = headless_message_failure_detail(
                info,
                interaction_id,
                input_mode,
                queue_policy,
                "lease_unavailable",
                error,
            );
            persist_interaction_delivery_attempt(
                state,
                interaction_id,
                &info.uuid,
                DeliveryTransportKind::HeadlessProcess,
                &detail,
            )
            .await;
            record_delivery_attempt(state, &detail).await;
            return HeadlessMessageDelivery::Completed(Box::new(detail));
        }
    };
    let mut lease_guard =
        wardian_core::conversation_lease::PersistedConversationLeaseGuard::new(&lease);
    let Some(_lifecycle_guard) = state.try_lock_agent_lifecycle(&info.uuid).await else {
        return HeadlessMessageDelivery::Busy(Box::new(
            delivery_target_info(state, &info.uuid)
                .await
                .unwrap_or_else(|_| info.clone()),
        ));
    };
    let current_info = match delivery_target_info(state, &info.uuid).await {
        Ok(current_info) => current_info,
        Err(error) => {
            let detail = headless_message_failure_detail(
                info,
                interaction_id,
                input_mode,
                queue_policy,
                "target_replaced",
                error.message,
            );
            persist_interaction_delivery_attempt(
                state,
                interaction_id,
                &info.uuid,
                DeliveryTransportKind::HeadlessProcess,
                &detail,
            )
            .await;
            record_delivery_attempt(state, &detail).await;
            return HeadlessMessageDelivery::Completed(Box::new(detail));
        }
    };
    if !same_delivery_target_incarnation(info, &current_info)
        || !status_uses_headless_delivery(&current_info.status)
    {
        return HeadlessMessageDelivery::Busy(Box::new(current_info));
    }
    record_headless_status_observation(app, state, &current_info).await;

    let result = crate::delivery::run_headless_process_prompt(
        state,
        crate::delivery::HeadlessProcessPromptRequest {
            node: "message_delivery".to_string(),
            provider: current_info.provider.clone(),
            cwd: current_info.cwd.clone(),
            prompt: prompt.to_string(),
            session_id: current_info.uuid.clone(),
            resume_session: current_info.resume_session.clone(),
            config_override: Some(current_info.config.clone()),
            interaction_id: Some(interaction_id.to_string()),
            timeout,
            lease_owner: Some(lease_guard.owner().clone()),
        },
    )
    .await;

    match result {
        Ok(result) => {
            record_headless_message_response(
                state,
                &current_info,
                interaction_id,
                &result.response,
            )
            .await;
            record_headless_message_exchange(
                state,
                &current_info,
                interaction_id,
                prompt,
                &result.response,
                origin,
            )
            .await;
            let mut detail = DeliveryDetail {
                uuid: current_info.uuid.clone(),
                name: current_info.name.clone(),
                provider: current_info.provider.clone(),
                runtime_state: "headless_process".to_string(),
                delivery_state: "provider_applied".to_string(),
                input_mode,
                queue_policy,
                message_id: Some(interaction_id.to_string()),
                delivery_phase: Some("process_completed".to_string()),
                observed_state: Some("stdout_parsed".to_string()),
                reason: Some("target was not live; ran provider headlessly".to_string()),
                profile: Some(
                    crate::utils::delivery_profile::delivery_profile(&current_info.provider)
                        .provider,
                ),
                error: None,
            };
            record_delivery_attempt(state, &detail).await;
            let release_error = lease_guard.release().err();
            if let Some(error) = release_error {
                detail.reason = Some(format!(
                    "target was not live; ran provider headlessly (lease cleanup is pending until it can be released or expires: {error})"
                ));
            } else {
                record_headless_status_observation(app, state, &current_info).await;
            }
            HeadlessMessageDelivery::Completed(Box::new(detail))
        }
        Err(error) => {
            let diagnostic =
                crate::delivery::headless_process::sanitize_headless_error(&error, prompt);
            let mut detail = headless_message_failure_detail(
                &current_info,
                interaction_id,
                input_mode,
                queue_policy,
                "headless_process_failed",
                diagnostic,
            );
            // The process runner already persisted this attempt. This watch
            // record is intentionally not another durable delivery attempt.
            record_delivery_attempt(state, &detail).await;
            let release_error = lease_guard.release().err();
            if let Some(release_error) = release_error {
                if let Some(error) = detail.error.as_mut() {
                    error.message.push_str(&format!(
                        "; additionally failed to release the conversation lease: {release_error}"
                    ));
                }
            } else {
                record_headless_status_observation(app, state, &current_info).await;
            }
            HeadlessMessageDelivery::Completed(Box::new(detail))
        }
    }
}

fn acquire_headless_message_lease(
    info: &DeliveryTargetInfo,
    interaction_id: &str,
) -> Result<wardian_core::conversation_lease::ConversationLease, HeadlessMessageLeaseError> {
    let now = chrono::Utc::now();
    let now_rfc3339 = now.to_rfc3339();
    let resume_session = info
        .resume_session
        .as_deref()
        .map(str::trim)
        .filter(|session| !session.is_empty())
        .unwrap_or_default()
        .to_string();
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: info.uuid.clone(),
        provider: info.provider.clone(),
        resume_session: resume_session.clone(),
        owner_kind: "message_delivery".to_string(),
        owner_id: interaction_id.to_string(),
        acquisition_id: uuid::Uuid::new_v4().to_string(),
        owner_node_id: None,
        mode: if resume_session.is_empty() {
            "background_fresh".to_string()
        } else {
            "background_resume".to_string()
        },
        started_at: now_rfc3339.clone(),
        heartbeat_at: now_rfc3339.clone(),
        expires_at: (now + chrono::Duration::minutes(20)).to_rfc3339(),
    };
    match wardian_core::conversation_lease::try_acquire_lease(lease.clone(), &now_rfc3339) {
        Ok(wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Acquired) => {
            Ok(lease)
        }
        Ok(wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Conflict(_)) => {
            Err(HeadlessMessageLeaseError::Busy)
        }
        Err(error) => Err(HeadlessMessageLeaseError::Failed(error)),
    }
}

fn headless_message_failure_detail(
    info: &DeliveryTargetInfo,
    interaction_id: &str,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    code: &str,
    message: impl Into<String>,
) -> DeliveryDetail {
    DeliveryDetail {
        uuid: info.uuid.clone(),
        name: info.name.clone(),
        provider: info.provider.clone(),
        runtime_state: "headless_process".to_string(),
        delivery_state: "failed".to_string(),
        input_mode,
        queue_policy,
        message_id: Some(interaction_id.to_string()),
        delivery_phase: Some("process_failed".to_string()),
        observed_state: None,
        reason: Some("target was not live; headless provider execution failed".to_string()),
        profile: Some(crate::utils::delivery_profile::delivery_profile(&info.provider).provider),
        error: Some(DeliveryErrorDetail {
            code: code.to_string(),
            message: message.into(),
        }),
    }
}

/// Records the lease-derived status that the roster, telemetry, and CLI
/// snapshots expose during a headless run. The underlying persisted status is
/// left intact: a completed run returns an offline agent to `off` rather than
/// inventing a live `idle` session.
async fn record_headless_status_observation(
    app: Option<&AppHandle>,
    state: &AppState,
    info: &DeliveryTargetInfo,
) {
    let observed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let agents = state.agents.lock().await;
    let Some(agent) = agents.get(&info.uuid) else {
        return;
    };
    if !delivery_target_matches_current_agent(agent, info) {
        return;
    };
    let status = snapshot_agent(agent).status;
    if let Ok(mut last_status_at) = agent.last_status_at.lock() {
        *last_status_at = Some(observed_at.clone());
    }
    if let Ok(mut watch_state) = agent.watch_state.lock() {
        watch_state.push_event(
            "status",
            serde_json::json!({
                "status": status,
                "observed_at": observed_at,
                "source": "headless_process",
            }),
        );
    };
    drop(agents);

    if let Some(app) = app {
        let display_status = display_status_for_agent_event(&status);
        let _ = app.emit(
            "agent-status-updated",
            serde_json::json!({
                "session_id": info.uuid,
                "current_status": display_status,
            }),
        );
    }
}

async fn record_headless_message_response(
    state: &AppState,
    info: &DeliveryTargetInfo,
    interaction_id: &str,
    response: &str,
) {
    let agents = state.agents.lock().await;
    let Some(agent) = agents.get(&info.uuid) else {
        return;
    };
    if !delivery_target_matches_current_agent(agent, info) {
        manager::log_debug(&format!(
            "[WARDIAN] ignoring stale headless response for replaced agent {}",
            info.uuid
        ));
        return;
    }
    let Ok(mut watch_state) = agent.watch_state.lock() else {
        return;
    };
    watch_state.push_output(format!("{response}\r\n").as_bytes());
    watch_state.push_transcript(wardian_core::control::WatchTranscriptMessage {
        role: "assistant".to_string(),
        text: response.to_string(),
        provider: info.provider.clone(),
        turn_id: Some(interaction_id.to_string()),
        source: Some("headless_process".to_string()),
    });
}

async fn record_headless_message_exchange(
    state: &AppState,
    info: &DeliveryTargetInfo,
    interaction_id: &str,
    prompt: &str,
    response: &str,
    origin: Option<&MessageOrigin>,
) {
    let is_current = {
        let agents = state.agents.lock().await;
        agents
            .get(&info.uuid)
            .is_some_and(|agent| delivery_target_matches_current_agent(agent, info))
    };
    if !is_current {
        manager::log_debug(&format!(
            "[WARDIAN] ignoring stale headless conversation archive for replaced agent {}",
            info.uuid
        ));
        return;
    }
    let global_conversation_logging = crate::utils::shell::load_shell_settings()
        .unwrap_or_default()
        .conversation_logging;
    if effective_conversation_logging(
        global_conversation_logging,
        info.config.conversation_logging,
    ) != ConversationLoggingSetting::Enabled
    {
        return;
    }

    let context = headless_conversation_archive_context(info);
    let provider_session_id = context.provider_session_ids.first().cloned();
    let sender_agent_id = origin.map(|MessageOrigin::WardianAgent { session_id }| session_id);
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let source = Some("headless_process".to_string());
    let events = vec![
        AgentChatEvent {
            id: format!("headless:{interaction_id}:user"),
            session_id: info.uuid.clone(),
            provider: info.provider.clone(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::User),
            text: Some(prompt.to_string()),
            title: None,
            status: None,
            turn_id: Some(interaction_id.to_string()),
            source: source.clone(),
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: Some(created_at.clone()),
            sequence: None,
            metadata: serde_json::json!({
                "provider_session_id": provider_session_id,
                "headless": true,
                "interaction_id": interaction_id,
                "sender_agent_id": sender_agent_id,
            }),
        },
        AgentChatEvent {
            id: format!("headless:{interaction_id}:assistant"),
            session_id: info.uuid.clone(),
            provider: info.provider.clone(),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            text: Some(response.to_string()),
            title: None,
            status: None,
            turn_id: Some(interaction_id.to_string()),
            source,
            command: None,
            exit_code: None,
            path: None,
            language: None,
            created_at: Some(created_at),
            sequence: None,
            metadata: serde_json::json!({
                "provider_session_id": context.provider_session_ids.first(),
                "headless": true,
                "interaction_id": interaction_id,
            }),
        },
    ];
    let agent_id = context.agent_id.clone();
    if let Err(error) = state
        .conversation_archive
        .append_chat_events_with_context(context, &events)
    {
        manager::log_debug(&format!(
            "[WARDIAN] headless conversation archive append failed for {agent_id}: {error}"
        ));
    }
}

fn headless_conversation_archive_context(info: &DeliveryTargetInfo) -> ConversationArchiveContext {
    let config = &info.config;
    let workspace = config
        .git_worktree_folder
        .clone()
        .unwrap_or_else(|| config.folder.clone());
    let provider_session_ids = [
        config.resume_session.as_deref(),
        config.fresh_provider_session_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(ToString::to_string)
    .collect::<Vec<_>>();
    let provider_source_key = provider_session_ids
        .first()
        .map(|session| format!("{}:session:{session}", config.provider));

    ConversationArchiveContext {
        agent_id: info.uuid.clone(),
        agent_name: if config.session_name.trim().is_empty() {
            info.uuid.clone()
        } else {
            config.session_name.clone()
        },
        agent_class: config.agent_class.clone(),
        workspace,
        provider: config.provider.clone(),
        provider_session_ids,
        provider_source_key,
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
    } else if info.provider == "prime" {
        // Prime's status never leaves its spawn value on the interactive path,
        // because the TUI emits nothing parse_output can read, so the generic
        // "is the agent idle" fallback below would never let a message through.
        wait_for_terminal_output(state, &info.uuid, 15_000, prime_output_has_ready_prompt).await
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

fn active_conversation_lease_for_delivery(info: &DeliveryTargetInfo) -> bool {
    let leases = wardian_core::conversation_lease::load_leases();
    wardian_core::conversation_lease::find_active_conflict(
        &leases,
        &info.uuid,
        info.resume_session.as_deref().unwrap_or_default(),
        &chrono::Utc::now().to_rfc3339(),
    )
    .is_some()
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

/// Captures the watch position before a native terminal submission. A later
/// `turn_started` event is emitted only from provider output, so it proves the
/// provider accepted a newly submitted prompt rather than merely rendering it
/// in the terminal composer.
pub(crate) async fn provider_turn_start_cursor(
    state: &AppState,
    session_id: &str,
) -> Result<String, String> {
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .ok_or_else(|| format!("Agent {session_id} not found or is off"))?
            .watch_state
            .clone()
    };
    watch_state
        .lock()
        .map(|watch_state| watch_state.latest_cursor())
        .map_err(|_| format!("Agent {session_id} watch state lock poisoned"))
}

/// Waits for provider output that starts a turn after a native terminal submit
/// key was written. The timeout is deliberately a delivery failure, not a
/// retry trigger: at that point the composer may still contain the payload.
pub(crate) async fn wait_for_provider_turn_started_after_submit(
    state: &AppState,
    session_id: &str,
    since_cursor: &str,
) -> Result<(), String> {
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .ok_or_else(|| format!("Agent {session_id} not found or is off"))?
            .watch_state
            .clone()
    };
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(PROVIDER_TURN_START_TIMEOUT_MS) {
        let snapshot = watch_state
            .lock()
            .map_err(|_| format!("Agent {session_id} watch state lock poisoned"))?
            .snapshot_since(Some(since_cursor), Some(0))
            .map_err(|error| format!("watch state error: {}", error.code()))?;
        if snapshot
            .events
            .iter()
            .any(|event| event.kind == "turn_started")
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    Err(format!(
        "Timed out waiting for {session_id} provider turn start after terminal submit"
    ))
}

fn codex_output_has_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    if codex_output_has_workspace_trust_prompt(&cleaned) {
        return false;
    }
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

fn codex_output_has_workspace_trust_prompt(output: &str) -> bool {
    output
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
        .contains("do you trust the contents of this directory?")
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
    if gemini_output_has_api_key_prompt(&cleaned) {
        return false;
    }
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

fn gemini_output_has_api_key_prompt(output: &str) -> bool {
    output.contains("Enter Gemini API Key") || output.contains("Paste your API key here")
}

pub(crate) fn antigravity_output_has_ready_prompt(output: &str) -> bool {
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

/// Words Prime Agent's TUI puts beside its spinner while a turn is running.
///
/// Prime Agent 0.7.0 renders `<spinner> Waiting · 3s`, switching the word as
/// the turn progresses (`Thinking`, `Writing`) and appending token counts. The
/// separator is what distinguishes the status line from the same word appearing
/// in agent output.
const PRIME_WORKING_WORDS: [&str; 4] = ["Waiting", "Thinking", "Writing", "Running"];

/// True when Prime Agent's TUI is showing a turn in progress.
///
/// This is the only turn-start signal the interactive transport has: the TUI
/// publishes no structured events, so without it a delivered message is
/// recorded as failed even though the model answered it.
pub(crate) fn prime_output_is_working(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    cleaned
        .lines()
        .map(str::trim)
        .any(prime_working_status_line)
}

fn prime_working_status_line(line: &str) -> bool {
    PRIME_WORKING_WORDS.iter().any(|word| {
        line.split_once(word)
            // `Thinking · 2s`, not a message that merely mentions thinking.
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('·'))
    })
}

/// True when Prime Agent's TUI is back at its idle prompt.
///
/// The footer carries `? for shortcuts` only while input is accepted; during a
/// turn Prime renders the same footer without it.
pub(crate) fn prime_output_has_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    let lines = cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    // Scan back from the end: an idle footer earlier in the buffer says
    // nothing about the state Prime is in now.
    for line in lines.iter().rev().take(8) {
        if prime_working_status_line(line) {
            return false;
        }
        if line.contains("? for shortcuts") {
            return true;
        }
    }
    false
}

pub(crate) async fn mark_delivered_agents_prompt_started(
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
    let _ = app.emit("pair-activity-changed", ());
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
    let delivery = match deliver_message_to_target_with_headless_timeout(
        Some(app),
        &state,
        target,
        &structured_delivery.prompt,
        thread,
        MessageInputMode::Message,
        QueuePolicy::QueueIfBusy,
        None,
        origin,
        false,
        timeout.min(MAX_HEADLESS_DELIVERY_TIMEOUT),
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

/// Delivers every valid request before waiting for any reply. Each pending request
/// shares one deadline; a timeout is recorded as a terminal failed reply so a
/// later `wardian reply` cannot revive an expired correlation.
async fn handle_structured_ask_many(
    app: &AppHandle,
    targets: &[String],
    message: &str,
    thread: Option<&str>,
    tail_bytes: Option<usize>,
    timeout: Duration,
    origin: Option<&MessageOrigin>,
) -> Result<String, ControlError> {
    validate_send_message_thread(thread)?;
    if targets.len() < 2 {
        return Err(ControlError::bad_request(
            "multi-target ask requires at least two explicit targets",
        ));
    }

    let state = app.state::<AppState>();
    let wardian_home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| ControlError::request_failed("could not resolve Wardian home"))?;
    let sender_session_id =
        origin.map(|MessageOrigin::WardianAgent { session_id }| session_id.clone());
    let mut pending = Vec::new();
    let mut results = Vec::with_capacity(targets.len());

    for target in targets {
        validate_watch_target(target)?;
        let Some(target_uuid) = resolve_target_uuid_in_state(&state, target).await else {
            results.push(ask_target_failure(
                target,
                AskTargetOutcome::DeliveryFailed,
                "not_found",
                format!("agent not found: {target}"),
            ));
            continue;
        };
        let watch_state = match agent_watch_state(&state, &target_uuid).await {
            Ok(watch_state) => watch_state,
            Err(error) => {
                results.push(ask_target_failure(
                    target,
                    AskTargetOutcome::DeliveryFailed,
                    error.code(),
                    error.to_string(),
                ));
                continue;
            }
        };
        let initial_cursor = match watch_state.lock() {
            Ok(watch) => watch.latest_cursor(),
            Err(_) => {
                results.push(ask_target_failure(
                    target,
                    AskTargetOutcome::DeliveryFailed,
                    "request_failed",
                    "watch state lock poisoned".to_string(),
                ));
                continue;
            }
        };
        let request_id = new_ask_request_id();
        let structured_delivery = match build_structured_ask_delivery_message(
            &wardian_home,
            &target_uuid,
            message,
            &request_id,
        ) {
            Ok(delivery) => delivery,
            Err(error) => {
                results.push(ask_target_failure(
                    target,
                    AskTargetOutcome::DeliveryFailed,
                    error.code(),
                    error.to_string(),
                ));
                continue;
            }
        };
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
                sender_session_id.clone(),
                target_uuid.clone(),
                body_ref,
            )
            .await;
        let _ = app.emit("pair-activity-changed", ());
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

        match deliver_message_to_target_with_headless_timeout(
            Some(app),
            &state,
            target,
            &structured_delivery.prompt,
            thread,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            origin,
            false,
            timeout.min(MAX_HEADLESS_DELIVERY_TIMEOUT),
        )
        .await
        {
            Ok(delivery) => pending.push((
                target.clone(),
                target_uuid,
                request_id,
                delivery,
                watch_state,
                initial_cursor,
            )),
            Err(error) => {
                let reply = fail_structured_ask_request(
                    &state,
                    &request_id,
                    &target_uuid,
                    &format!("delivery failed: {error}"),
                    Some(app),
                )
                .await;
                results.push(AskTargetResponse {
                    target: target.clone(),
                    request_id: Some(request_id),
                    outcome: AskTargetOutcome::DeliveryFailed,
                    delivery: Vec::new(),
                    reply,
                    watch: None,
                    watch_error: None,
                    failure: Some(WatchEvidenceError {
                        code: error.code().to_string(),
                        message: error.to_string(),
                    }),
                });
            }
        }
    }

    let deadline = std::time::Instant::now() + timeout;
    for (target, target_uuid, request_id, delivery, watch_state, initial_cursor) in pending {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let reply = wait_for_structured_reply(&state, &request_id, remaining).await;
        let fallback_agent = ask_fallback_agent_snapshot(&state, &target_uuid, &target).await;
        let watch_result = structured_ask_watch_response(
            &state,
            &target_uuid,
            watch_state,
            &initial_cursor,
            tail_bytes,
        )
        .await;
        let (outcome, reply, failure) = match reply {
            Ok(reply) => (AskTargetOutcome::Completed, Some(reply), None),
            Err(error) if error.code() == "watch_timeout" => {
                let reply = fail_structured_ask_request(
                    &state,
                    &request_id,
                    &target_uuid,
                    "structured reply timed out",
                    Some(app),
                )
                .await;
                (
                    AskTargetOutcome::TimedOut,
                    reply,
                    Some(WatchEvidenceError {
                        code: error.code().to_string(),
                        message: error.to_string(),
                    }),
                )
            }
            Err(error) => {
                let reply = fail_structured_ask_request(
                    &state,
                    &request_id,
                    &target_uuid,
                    &format!("ask cancelled: {error}"),
                    Some(app),
                )
                .await;
                (
                    AskTargetOutcome::Cancelled,
                    reply,
                    Some(WatchEvidenceError {
                        code: error.code().to_string(),
                        message: error.to_string(),
                    }),
                )
            }
        };
        let (watch, watch_error) = ask_watch_parts(fallback_agent, watch_result);
        results.push(AskTargetResponse {
            target,
            request_id: Some(request_id),
            outcome,
            delivery,
            reply,
            watch: Some(watch),
            watch_error,
            failure,
        });
    }

    ok_json(&AskManyResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        targets: results,
    })
}

fn ask_target_failure(
    target: &str,
    outcome: AskTargetOutcome,
    code: &str,
    message: String,
) -> AskTargetResponse {
    AskTargetResponse {
        target: target.to_string(),
        request_id: None,
        outcome,
        delivery: Vec::new(),
        reply: None,
        watch: None,
        watch_error: None,
        failure: Some(WatchEvidenceError {
            code: code.to_string(),
            message,
        }),
    }
}

fn ask_watch_parts(
    fallback_agent: WatchAgentSnapshot,
    watch_result: Result<AgentWatchResponse, ControlError>,
) -> (AgentWatchResponse, Option<WatchEvidenceError>) {
    match watch_result {
        Ok(watch) => (watch, None),
        Err(error) => (
            minimal_ask_watch_response(fallback_agent),
            Some(WatchEvidenceError {
                code: error.code().to_string(),
                message: error.to_string(),
            }),
        ),
    }
}

async fn fail_structured_ask_request(
    state: &AppState,
    request_id: &str,
    target_session_id: &str,
    body: &str,
    app: Option<&AppHandle>,
) -> Option<StructuredReply> {
    let reply = state
        .interactions
        .fail_task_with_reply(request_id, target_session_id, body)
        .await
        .ok()?;
    if let Some(app) = app {
        let _ = app.emit("pair-activity-changed", ());
    }
    let _ = push_watch_event_for_agent(
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
    .await;
    Some(reply)
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
        "{message}\n\nWardian request id: {request_id}\nWhen finished, execute this command from your shell/tool with the reply body on stdin:\nwardian reply {request_id} --status done --stdin\nUse --status blocked or --status failed if you cannot complete it. Do not print the command as your final answer; run it so Wardian can record the structured reply."
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
    app: Option<&AppHandle>,
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

        if let Some(app) = app {
            let _ = app.emit("pair-activity-changed", ());
        }

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
    // A conditional watch answers whether a new observation satisfies the
    // condition. An unanchored snapshot intentionally still returns retained
    // history, but treating retained history as a completion signal made
    // `--until status:idle` succeed on an old idle blip.
    let since = {
        let guard = watch_state
            .lock()
            .map_err(|_| ControlError::request_failed("watch state lock poisoned"))?;
        watch_start_cursor(&guard, options.since, condition.is_some())
    };
    let snapshot = if let Some(condition) = condition {
        wait_for_watch_condition(
            watch_state,
            since,
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
            .snapshot_since(since.as_deref(), options.tail_bytes)
            .map_err(control_error_from_watch_state)?
    };
    let agent = watch_agent_snapshot(&state, &uuid).await?;
    let includes = WatchIncludes::from_values(&options.include);

    ok_json(&build_agent_watch_response(agent, snapshot, &includes))
}

fn watch_start_cursor(
    state: &crate::state::AgentWatchState,
    requested_since: Option<String>,
    has_condition: bool,
) -> Option<String> {
    requested_since.or_else(|| has_condition.then(|| state.latest_cursor()))
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
    resume_session: Option<String>,
    cwd: PathBuf,
    config: wardian_core::models::AgentConfig,
    /// Stable Arc identity for the active-agent incarnation this delivery was
    /// resolved against. Clear, resume, and re-create replace the Arc, so a
    /// late headless completion cannot write into the successor agent.
    config_identity: Arc<Mutex<wardian_core::models::AgentConfig>>,
    status: String,
}

async fn delivery_target_infos(
    state: &AppState,
    session_ids: &[String],
) -> Result<Vec<DeliveryTargetInfo>, ControlError> {
    let mut infos = Vec::with_capacity(session_ids.len());
    for session_id in session_ids {
        infos.push(delivery_target_info(state, session_id).await?);
    }
    Ok(infos)
}

async fn delivery_target_info(
    state: &AppState,
    session_id: &str,
) -> Result<DeliveryTargetInfo, ControlError> {
    let agents = state.agents.lock().await;
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
        uuid: session_id.to_string(),
        name: config.session_name.clone(),
        provider: config.provider.clone(),
        resume_session: config.resume_session.clone(),
        cwd: PathBuf::from(&config.folder),
        config: config.clone(),
        config_identity: agent.config.clone(),
        status: normalize_status(&status),
    })
}

fn same_delivery_target_incarnation(left: &DeliveryTargetInfo, right: &DeliveryTargetInfo) -> bool {
    left.uuid == right.uuid && Arc::ptr_eq(&left.config_identity, &right.config_identity)
}

fn delivery_target_matches_current_agent(
    agent: &crate::state::ActiveAgent,
    info: &DeliveryTargetInfo,
) -> bool {
    Arc::ptr_eq(&agent.config, &info.config_identity)
}

fn display_status_for_agent_event(status: &str) -> &'static str {
    match status {
        "headless" => "Headless",
        "idle" => "Idle",
        "processing" => "Processing...",
        "action_required" => "Action Needed",
        _ => "Off",
    }
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

async fn record_conversation_delivery(
    state: &AppState,
    delivery: &[DeliveryDetail],
    message: &str,
    origin: Option<&MessageOrigin>,
) {
    if message.trim().is_empty() {
        return;
    }

    let global_conversation_logging = crate::utils::shell::load_shell_settings()
        .unwrap_or_default()
        .conversation_logging;
    let sender_agent_id =
        origin.map(|MessageOrigin::WardianAgent { session_id }| session_id.as_str());
    let target_settings = {
        let agents = state.agents.lock().await;
        delivery
            .iter()
            .filter(|detail| conversation_delivery_state_is_recordable(&detail.delivery_state))
            .filter_map(|detail| {
                let agent = agents.get(&detail.uuid)?;
                let config = agent.config.lock().ok()?;
                let setting = config.conversation_logging;
                let workspace = config
                    .git_worktree_folder
                    .clone()
                    .unwrap_or_else(|| config.folder.clone());
                let provider_session_ids = [
                    config.resume_session.as_deref(),
                    config.fresh_provider_session_id.as_deref(),
                ]
                .into_iter()
                .flatten()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
                let log_path =
                    agent.log_path.lock().ok().and_then(|path| {
                        path.as_ref().map(|path| path.to_string_lossy().to_string())
                    });
                let provider_source_key = provider_session_ids
                    .first()
                    .map(|session| format!("{}:session:{session}", config.provider))
                    .or_else(|| log_path.map(|path| format!("{}:source:{path}", config.provider)));
                let context = ConversationArchiveContext {
                    agent_id: detail.uuid.clone(),
                    agent_name: if config.session_name.trim().is_empty() {
                        detail.uuid.clone()
                    } else {
                        config.session_name.clone()
                    },
                    agent_class: config.agent_class.clone(),
                    workspace,
                    provider: config.provider.clone(),
                    provider_session_ids,
                    provider_source_key,
                };
                Some((context, setting))
            })
            .collect::<Vec<_>>()
    };

    for (context, agent_conversation_logging) in target_settings {
        if effective_conversation_logging(global_conversation_logging, agent_conversation_logging)
            != ConversationLoggingSetting::Enabled
        {
            continue;
        }
        let agent_id = context.agent_id.clone();
        if let Err(error) = state
            .conversation_archive
            .append_delivered_input_with_context(context, message, sender_agent_id)
        {
            manager::log_debug(&format!(
                "[WARDIAN] conversation archive delivery append failed for {agent_id}: {error}"
            ));
        }
    }
}

fn conversation_delivery_state_is_recordable(delivery_state: &str) -> bool {
    matches!(
        delivery_state,
        "submitted" | "submit_sent_unverified" | "provider_accepted" | "approval_submitted"
    )
}

async fn persist_interaction_delivery_attempt(
    state: &AppState,
    interaction_id: &str,
    target_session_id: &str,
    transport: DeliveryTransportKind,
    detail: &DeliveryDetail,
) {
    state
        .interactions
        .record_delivery_attempt(
            interaction_id,
            target_session_id,
            transport,
            state
                .interactions
                .current_provider_input_generation(target_session_id)
                .await
                .unwrap_or(0),
            &detail.runtime_state,
            &detail.delivery_state,
            detail.delivery_phase.clone(),
            detail.observed_state.clone(),
            detail.reason.clone(),
            detail.error.clone(),
        )
        .await;
}

pub(crate) async fn wait_for_terminal_ready_for_delivery_service(
    state: &AppState,
    session_id: &str,
) -> Result<(), String> {
    let info = delivery_target_infos(state, &[session_id.to_string()])
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| format!("agent not found: {session_id}"))?;
    wait_for_terminal_ready_for_control_send(state, &info).await
}

pub(crate) async fn submit_approval_action_for_delivery_service<S>(
    tx: &S,
    provider: &str,
    action: &ApprovalAction,
) -> Result<
    crate::utils::delivery_transaction::TerminalDeliveryOutcome,
    crate::utils::delivery_transaction::TerminalDeliveryError,
>
where
    S: crate::utils::delivery_transaction::TerminalInputSink + ?Sized,
{
    submit_approval_action_via_sender(tx, provider, action).await
}

pub(crate) async fn push_delivery_for_delivery_service(
    state: &AppState,
    session_id: &str,
    detail: &DeliveryDetail,
) {
    let agents = state.agents.lock().await;
    if let Some(agent) = agents.get(session_id) {
        if let Ok(mut watch_state) = agent.watch_state.lock() {
            watch_state.push_delivery(serde_json::json!(detail));
        }
    }
}

pub(crate) async fn mark_delivered_agents_prompt_started_for_delivery_service(
    app: Option<&AppHandle>,
    state: &AppState,
    session_ids: &[String],
) {
    mark_delivered_agents_prompt_started(app, state, session_ids).await;
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

/// Gives restored durable mailbox work one immediate, status-gated chance to
/// drain. Later provider idle observations remain the normal delivery trigger;
/// this does not poll or retry terminal input.
pub(crate) fn spawn_mailbox_drain_after_restore(app: &AppHandle, session_id: &str) {
    let app = app.clone();
    let session_id = session_id.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _ = drain_next_mailbox_message_for_idle_agent(Some(&app), &state, &session_id).await;
    });
}

pub(crate) async fn drain_mailbox_for_idle_agent_from_status_observation(
    app: Option<&AppHandle>,
    state: &AppState,
    session_id: &str,
) {
    let _ = drain_next_mailbox_message_for_idle_agent(app, state, session_id).await;
}

async fn drain_next_mailbox_message_for_idle_agent(
    app: Option<&AppHandle>,
    state: &AppState,
    session_id: &str,
) -> Result<Option<DeliveryDetail>, ControlError> {
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
    if active_conversation_lease_for_delivery(&info) {
        return Ok(None);
    }

    let record = {
        let mut mailbox = state.mailbox.lock().await;
        mailbox.take_next_pending_for_target(session_id)
    };
    let Some(record) = record else {
        return Ok(None);
    };

    let dispatch_persist_error = wardian_core::db::upsert_mailbox_message(&record)
        .err()
        .map(|error| error.to_string());
    if let Some(error) = dispatch_persist_error {
        state.mailbox.lock().await.mark_pending(&record.id);
        return Err(ControlError::request_failed(format!(
            "failed to persist mailbox dispatch state: {error}"
        )));
    }
    if let Err(error) = state
        .interactions
        .update_message_status_durable(&record.interaction_id, InteractionStatus::Delivering)
        .await
    {
        let requeued = state.mailbox.lock().await.mark_pending(&record.id);
        if let Some(requeued) = requeued {
            let _ = wardian_core::db::upsert_mailbox_message(&requeued);
        }
        return Err(ControlError::request_failed(error));
    }

    let target_uuid = info.uuid.clone();
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
        profile: Some(crate::utils::delivery_profile::delivery_profile(&info.provider).provider),
        error: None,
    };
    let result = crate::delivery::submit_live_surface_prompt(
        app,
        state,
        crate::delivery::LiveSurfacePromptRequest {
            session_id: session_id.to_string(),
            prompt: record.body.clone(),
            interaction_id: Some(record.interaction_id.clone()),
            input_mode: record.input_mode,
            queue_policy: record.queue_policy,
            approval_action: record.approval_action.clone(),
            origin: record.origin.clone(),
            runtime_state: "mailbox_drain",
            mark_prompt_started: true,
            payload_sent_detail: Some(submit_started),
            delivery_message_id: Some(record.id.clone()),
        },
    )
    .await;

    let detail = match result {
        Ok(result) => {
            state.mailbox.lock().await.mark_delivered(&record.id);
            let _ = wardian_core::db::delete_mailbox_message(&record.id);
            let _ = state
                .interactions
                .update_message_status_durable(&record.interaction_id, InteractionStatus::Delivered)
                .await;
            let mut detail = result.detail;
            detail.message_id = Some(record.id.clone());
            detail
        }
        Err(error) => {
            let missing_ready_input_channel = error.detail.as_ref().is_some_and(|detail| {
                detail
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code == "no_input_channel")
            }) && matches!(
                provider_input_current_state(state, session_id).await,
                Some(ProviderInputReadiness::Ready)
            );
            let retry_safe = error.retry_safe && !missing_ready_input_channel;
            if retry_safe {
                let requeued = state.mailbox.lock().await.mark_pending(&record.id);
                if let Some(requeued) = requeued {
                    let _ = wardian_core::db::upsert_mailbox_message(&requeued);
                }
                let _ = state
                    .interactions
                    .update_message_status_durable(
                        &record.interaction_id,
                        InteractionStatus::Queued,
                    )
                    .await;
            } else {
                let terminal = state.mailbox.lock().await.mark_failed(&record.id);
                if let Some(terminal) = terminal {
                    persist_terminal_mailbox_record(&terminal);
                }
                let _ = state
                    .interactions
                    .update_message_status_durable(
                        &record.interaction_id,
                        InteractionStatus::Failed,
                    )
                    .await;
            }
            let service_recorded_detail = error.detail.is_some();
            let mut detail = if let Some(detail) = error.detail {
                detail
            } else {
                failed_delivery_detail(
                    info,
                    "mailbox_drain",
                    "send_failed",
                    error.to_string(),
                    record.input_mode,
                    record.queue_policy,
                )
            };
            detail.message_id = Some(record.id.clone());
            if detail.delivery_phase.is_none() {
                detail.delivery_phase = Some(if retry_safe {
                    "queued".to_string()
                } else {
                    "terminal_state_unknown".to_string()
                });
            }
            detail.reason = Some(if retry_safe {
                "queued message remains pending until a new idle or ready observation".to_string()
            } else if missing_ready_input_channel {
                "agent reported ready but its input channel was unavailable; delivery stopped to prevent late delivery".to_string()
            } else {
                "queued message marked failed because terminal state is partial or unknown"
                    .to_string()
            });
            if !service_recorded_detail {
                persist_interaction_delivery_attempt(
                    state,
                    &record.interaction_id,
                    &target_uuid,
                    DeliveryTransportKind::LiveSurface,
                    &detail,
                )
                .await;
                record_delivery_attempt(state, &detail).await;
            }
            detail
        }
    };

    Ok(Some(detail))
}

fn persist_terminal_mailbox_record(record: &MailboxMessageRecord) {
    // Persist the terminal marker before removing the durable queue row. If a
    // shutdown lands between the two writes, startup still fails the in-flight
    // record rather than replaying an ambiguous terminal payload.
    if let Err(error) = wardian_core::db::upsert_mailbox_message(record) {
        manager::log_debug(&format!(
            "[Wardian] failed to persist terminal mailbox message {}: {error}",
            record.id
        ));
    }
    if let Err(error) = wardian_core::db::delete_mailbox_message(&record.id) {
        manager::log_debug(&format!(
            "[Wardian] failed to remove terminal mailbox message {}: {error}",
            record.id
        ));
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
            description: config.description.clone(),
            class: config.agent_class.clone(),
            provider: config.provider.clone(),
            status: "idle".to_string(),
            pid: None,
            started_at: None,
            workspace: (!config.folder.trim().is_empty()).then_some(config.folder.clone()),
            last_status_at: None,
            status_source: StatusSource::Live,
            visibility: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

fn validate_inbox_notification(
    notification: &InboxNotificationPayload,
) -> Result<(), ControlError> {
    let valid_text = |text: &str, max: usize| !text.trim().is_empty() && text.len() <= max;
    if !valid_text(&notification.title, 160) || !valid_text(&notification.body, 4_000) {
        return Err(ControlError::bad_request(
            "notification title and body must be non-empty and within their size limits",
        ));
    }
    match notification.kind {
        InboxNotificationKind::Update => {
            if notification.proposed_action.is_some()
                || notification.risk.is_some()
                || !notification.choices.is_empty()
                || notification.expires_at.is_some()
            {
                return Err(ControlError::bad_request(
                    "updates cannot include approval fields",
                ));
            }
        }
        InboxNotificationKind::Approval => {
            if !notification
                .proposed_action
                .as_deref()
                .is_some_and(|value| valid_text(value, 1_000))
                || !notification
                    .risk
                    .as_deref()
                    .is_some_and(|value| valid_text(value, 1_000))
                || notification.choices.len() < 2
                || notification.choices.len() > 5
                || notification
                    .choices
                    .iter()
                    .any(|choice| !valid_text(choice, 120))
                || notification
                    .expires_at
                    .as_deref()
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .is_none()
            {
                return Err(ControlError::bad_request(
                    "approvals require a proposed action, risk, two to five choices, and an expiry",
                ));
            }
        }
    }
    Ok(())
}

fn notification_control_error(error: &'static str) -> ControlError {
    match error {
        "approval_already_open" => ControlError::coded(
            "approval_already_open",
            "this agent already has an unresolved approval request",
        ),
        "persistence_failed" => {
            ControlError::coded("persistence_failed", "could not persist Inbox notification")
        }
        "invalid_notification" => ControlError::bad_request("invalid Inbox notification"),
        _ => ControlError::request_failed(error),
    }
}

fn ok_json<T: serde::Serialize>(value: &T) -> Result<String, ControlError> {
    serde_json::to_string(value).map_err(ControlError::request_failed)
}

fn artifact_store() -> Result<wardian_core::artifacts::ArtifactStore, ControlError> {
    let home = crate::utils::fs::get_wardian_home()
        .ok_or_else(|| ControlError::request_failed("Could not locate Wardian home"))?;
    wardian_core::artifacts::ArtifactStore::open(home.join("artifacts"))
        .map_err(ControlError::request_failed)
}

fn artifact_service_control_error(
    error: crate::artifact_service::ArtifactServiceError,
) -> ControlError {
    let code = match error.code.as_str() {
        "invalid_origin" => "invalid_origin",
        "unauthorized_path" => "unauthorized_path",
        "unreadable_file" => "unreadable_file",
        "unstable_file_timeout" => "unstable_file_timeout",
        "artifact_not_found" => "artifact_not_found",
        "review_not_found" => "review_not_found",
        "ui_delivery_failed" => "ui_delivery_failed",
        "invalid_request" => "bad_request",
        _ => "request_failed",
    };
    let persisted = error.persisted;
    let mut control = ControlError::coded(code, error.message);
    if let Some(persisted) = persisted {
        control = control.with_details(serde_json::json!({ "persisted": persisted }));
    }
    control
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
    let active_leases = wardian_core::conversation_lease::load_leases();
    let lease_now = chrono::Utc::now().to_rfc3339();
    let mut snapshots = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for session_id in order {
        if let Some(agent) = agents.get(&session_id) {
            snapshots.push(snapshot_agent_with_leases(
                agent,
                &active_leases,
                &lease_now,
            ));
            seen.insert(session_id);
        }
    }

    for (session_id, agent) in agents.iter() {
        if !seen.contains(session_id) {
            snapshots.push(snapshot_agent_with_leases(
                agent,
                &active_leases,
                &lease_now,
            ));
        }
    }

    snapshots
}

fn snapshot_agent(agent: &crate::state::ActiveAgent) -> AgentIdentity {
    let active_leases = wardian_core::conversation_lease::load_leases();
    let lease_now = chrono::Utc::now().to_rfc3339();
    snapshot_agent_with_leases(agent, &active_leases, &lease_now)
}

fn snapshot_agent_with_leases(
    agent: &crate::state::ActiveAgent,
    active_leases: &[wardian_core::conversation_lease::ConversationLease],
    lease_now: &str,
) -> AgentIdentity {
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
    let is_offline = config.is_off
        || matches!(
            wardian_core::identity::normalize_status(&status).as_str(),
            "off" | "error"
        );
    let effective_status = if is_offline
        && wardian_core::conversation_lease::find_active_execution_conflict(
            active_leases,
            &config.session_id,
            config.resume_session.as_deref().unwrap_or_default(),
            lease_now,
        )
        .is_some()
    {
        "Headless".to_string()
    } else {
        status
    };
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
        description: config.description,
        class: config.agent_class,
        provider: config.provider,
        status: normalize_status(&effective_status),
        pid: agent.process_id,
        started_at,
        workspace: (!config.folder.trim().is_empty()).then_some(config.folder),
        last_status_at,
        status_source: StatusSource::Live,
        visibility: None,
    }
}

fn agent_update_requires_restart(updated_fields: &[String], is_off: bool) -> bool {
    !is_off
        && updated_fields.iter().any(|field| {
            matches!(
                field.as_str(),
                "class" | "workspace" | "model" | "reasoning_effort"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ActiveAgent;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::{Arc, Mutex};
    use wardian_core::models::{
        AgentConfig, AgentConversationMode, BusyPolicy, WorkflowRoleAssignment,
    };

    struct TestWardianHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous_home: Option<OsString>,
        _temp: tempfile::TempDir,
    }

    impl TestWardianHome {
        fn new() -> Self {
            let lock = crate::utils::wardian_test_env_lock();
            let temp = tempfile::tempdir().expect("temp wardian home");
            let previous_home = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", temp.path());
            wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
                .expect("init test database");
            Self {
                _lock: lock,
                previous_home,
                _temp: temp,
            }
        }

        fn path(&self) -> &std::path::Path {
            self._temp.path()
        }
    }

    impl Drop for TestWardianHome {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    #[test]
    fn agent_description_update_does_not_require_restart() {
        assert!(!agent_update_requires_restart(
            &["description".to_string()],
            false
        ));
        assert!(agent_update_requires_restart(&["class".to_string()], false));
        assert!(!agent_update_requires_restart(
            &["workspace".to_string()],
            true
        ));
    }

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn node_available() -> bool {
        std::process::Command::new(if cfg!(windows) { "node.exe" } else { "node" })
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Regression test for the silent release-build crash where `claim_control_endpoint`
    /// was called from Tauri's `setup` hook (no Tokio runtime context), causing
    /// `tokio::net::windows::named_pipe::ServerOptions::create` to panic with
    /// "there is no reactor running". This test runs as a plain `#[test]` — *not*
    /// `#[tokio::test]` — so the absence of an ambient runtime mirrors the real
    /// setup-hook environment. The claim must succeed without panicking.
    #[test]
    fn control_endpoint_claim_succeeds_without_ambient_tokio_runtime() {
        let _home = TestWardianHome::new();

        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "test precondition: no Tokio runtime must be ambient on this thread, \
             otherwise we are not exercising the setup-hook code path"
        );

        let claim =
            claim_control_endpoint().expect("claim must not panic or fail outside a runtime");
        drop(claim);
    }

    #[tokio::test]
    async fn control_endpoint_claim_is_exclusive_for_current_home() {
        let _home = TestWardianHome::new();

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
    }

    #[test]
    fn workflow_run_control_launch_forwards_launch_options() {
        let mut assignments = wardian_core::models::WorkflowAssignments::new();
        assignments.insert(
            "reviewer".to_string(),
            WorkflowRoleAssignment::Agent {
                agent_id: "agent-1".to_string(),
                conversation: AgentConversationMode::Current,
                busy_policy: BusyPolicy::Fail,
            },
        );
        let bindings = HashMap::from([("legacy".to_string(), "mock".to_string())]);
        let input = serde_json::json!({"target":"HEAD"});
        let request = ControlRequest::WorkflowRun {
            path: "/workflow/controlwf.md".to_string(),
            provider: Some("mock".to_string()),
            workspace: Some("/workspace".to_string()),
            input: Some(input.clone()),
            bindings: Some(bindings.clone()),
            assignments: Some(assignments.clone()),
        };

        let launch = workflow_run_control_launch(request).unwrap();

        assert_eq!(launch.path, "/workflow/controlwf.md");
        assert_eq!(launch.provider.as_deref(), Some("mock"));
        assert_eq!(launch.workspace.as_deref(), Some("/workspace"));
        assert_eq!(launch.input, Some(input));
        assert_eq!(launch.bindings, Some(bindings));
        assert_eq!(launch.assignments, Some(assignments));
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
            runtime_generation: None,
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

    async fn install_test_terminal_runtime(
        state: &AppState,
        session_id: &str,
        input_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) {
        let generation = state
            .terminal_sessions
            .start_or_replace_runtime(
                session_id,
                crate::state::terminal_session::TerminalRuntimeHandles::new(input_tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
            )
            .await
            .expect("test terminal runtime");
        if let Some(agent) = state.agents.lock().await.get_mut(session_id) {
            agent.runtime_generation = Some(generation);
        }
    }

    async fn install_test_terminal_runtime_with_write_receipts(
        state: &AppState,
        session_id: &str,
        input_tx: tokio::sync::mpsc::Sender<
            crate::state::terminal_session::NativeTerminalWriteRequest,
        >,
    ) {
        let generation = state
            .terminal_sessions
            .start_or_replace_runtime(
                session_id,
                crate::state::terminal_session::TerminalRuntimeHandles::new_with_write_ack(
                    input_tx,
                    |_| Ok(()),
                ),
                wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
            )
            .await
            .expect("test terminal runtime");
        if let Some(agent) = state.agents.lock().await.get_mut(session_id) {
            agent.runtime_generation = Some(generation);
        }
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
    fn codex_ready_prompt_rejects_workspace_trust_modal() {
        assert!(!codex_output_has_ready_prompt(
            "\r\n› 1. Yes, continue\r\n  2. No, quit\r\n\r\nDo you trust the contents of this directory?\r\nPress enter to continue"
        ));
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
    fn gemini_ready_prompt_rejects_api_key_modal_over_composer() {
        assert!(!gemini_output_has_ready_prompt(
            "\r\n╭────────────────────────────────────────────────────────╮\r\n\
             │ Enter Gemini API Key                                  │\r\n\
             │ Paste your API key here                               │\r\n\
             ╰────────────────────────────────────────────────────────╯\r\n\
             \r\n\
             >   Type your message or @path/to/file\r\n\
             workspace (/directory)        Auto (Gemini 3)       2% used\r\n",
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"hello".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\x1b[13u".to_vec());
    }

    #[tokio::test]
    async fn native_codex_delivery_submits_without_waiting_for_payload_echo() {
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        record_provider_ready_evidence(&state, "agent-1", ProviderReadyEvidence::PromptDetected)
            .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime_with_write_receipts(&state, "agent-1", tx).await;

        let delivery = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "hello",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
            false,
        );
        tokio::pin!(delivery);

        let payload = tokio::select! {
            request = rx.recv() => request.expect("payload write request"),
            result = &mut delivery => panic!("delivery completed before payload write: {result:?}"),
        };
        assert_eq!(payload.bytes, b"\x1b[200~hello\x1b[201~".to_vec());
        payload.completion.send(Ok(())).expect("payload receipt");

        let submit = tokio::select! {
            request = rx.recv() => request.expect("submit write request"),
            result = &mut delivery => panic!("delivery completed before submit write: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                panic!("Codex submit must not wait for a terminal echo")
            }
        };
        assert_eq!(submit.bytes, b"\r".to_vec());
        submit.completion.send(Ok(())).expect("submit receipt");

        crate::manager::record_agent_turn_started_for_watch(&state, "agent-1").await;
        let delivery = delivery.await.expect("delivered after provider receipt");

        assert_eq!(delivery[0].delivery_state, "provider_accepted");
        assert_eq!(delivery[0].delivery_phase.as_deref(), Some("turn_started"));
    }

    #[tokio::test]
    async fn message_delivery_archives_unconfirmed_live_input_with_agent_origin() {
        let _home = TestWardianHome::new();
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: wardian_core::conversations::ConversationLoggingSetting::Disabled,
            ..Default::default()
        })
        .expect("save shell settings");
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().conversation_logging =
                wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        }
        let delivery = vec![DeliveryDetail {
            uuid: "agent-1".to_string(),
            name: "CoderOne".to_string(),
            provider: "mock".to_string(),
            runtime_state: "live_pty_available".to_string(),
            delivery_state: "provider_accepted".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            message_id: None,
            delivery_phase: Some("payload_sent".to_string()),
            observed_state: Some("bytes_sent".to_string()),
            reason: None,
            profile: None,
            error: None,
        }];

        record_conversation_delivery(
            &state,
            &delivery,
            "Review this change.",
            Some(&MessageOrigin::WardianAgent {
                session_id: "source-agent".to_string(),
            }),
        )
        .await;

        let conversation_id = state
            .conversation_archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_path =
            wardian_core::paths::agent_conversation_dir("agent-1", &conversation_id)
                .expect("conversation dir")
                .join("conversation.jsonl");
        let records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(&conversation_path)
                .expect("read records");

        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].kind,
            wardian_core::conversations::ConversationRecordKind::Message
        );
        assert_eq!(records[0].role.as_deref(), Some("user"));
        assert_eq!(
            records[0].speaker_type,
            Some(wardian_core::conversations::ConversationSpeakerType::Agent)
        );
        assert_eq!(records[0].text.as_deref(), Some("Review this change."));
    }

    #[tokio::test]
    async fn message_delivery_does_not_archive_agent_with_disabled_logging() {
        let _home = TestWardianHome::new();
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: wardian_core::conversations::ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("save shell settings");
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().conversation_logging =
                wardian_core::conversations::AgentConversationLoggingSetting::Disabled;
        }
        let delivery = vec![DeliveryDetail {
            uuid: "agent-1".to_string(),
            name: "CoderOne".to_string(),
            provider: "mock".to_string(),
            runtime_state: "live_pty_available".to_string(),
            delivery_state: "provider_accepted".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            message_id: None,
            delivery_phase: Some("payload_sent".to_string()),
            observed_state: Some("bytes_sent".to_string()),
            reason: None,
            profile: None,
            error: None,
        }];

        record_conversation_delivery(&state, &delivery, "Sensitive input.", None).await;

        assert!(state
            .conversation_archive
            .active_conversation_id_for_test("agent-1")
            .is_none());
    }

    #[tokio::test]
    async fn message_delivery_does_not_archive_queued_input() {
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let delivery = vec![DeliveryDetail {
            uuid: "agent-1".to_string(),
            name: "CoderOne".to_string(),
            provider: "mock".to_string(),
            runtime_state: "queued_not_live".to_string(),
            delivery_state: "queued".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            message_id: Some("msg-1".to_string()),
            delivery_phase: Some("queued".to_string()),
            observed_state: None,
            reason: Some(
                "queued message remains pending until a new idle or ready observation".to_string(),
            ),
            profile: None,
            error: None,
        }];

        record_conversation_delivery(&state, &delivery, "Queue this change.", None).await;

        assert!(state
            .conversation_archive
            .active_conversation_id_for_test("agent-1")
            .is_none());
    }

    #[test]
    fn generic_conversation_delivery_leaves_headless_exchanges_to_their_durable_recorder() {
        assert!(!conversation_delivery_state_is_recordable(
            "provider_applied"
        ));
    }

    #[tokio::test]
    async fn completed_headless_exchange_is_archived_once_with_provider_context() {
        let _home = TestWardianHome::new();
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: wardian_core::conversations::ConversationLoggingSetting::Disabled,
            ..Default::default()
        })
        .expect("save shell settings");
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").expect("agent");
            let mut config = agent.config.lock().expect("config");
            config.resume_session = Some("provider-session-1".to_string());
            config.conversation_logging =
                wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        }
        let info = delivery_target_infos(&state, &["agent-1".to_string()])
            .await
            .expect("delivery info")
            .remove(0);
        let origin = MessageOrigin::WardianAgent {
            session_id: "source-agent".to_string(),
        };

        record_headless_message_exchange(
            &state,
            &info,
            "interaction-1",
            "From Source: review this change.",
            "I reviewed it.",
            Some(&origin),
        )
        .await;
        // Stable event ids make a retry idempotent rather than duplicating the
        // user prompt or the provider response.
        record_headless_message_exchange(
            &state,
            &info,
            "interaction-1",
            "From Source: review this change.",
            "I reviewed it.",
            Some(&origin),
        )
        .await;

        let conversation_id = state
            .conversation_archive
            .active_conversation_id_for_test("agent-1")
            .expect("active conversation id");
        let conversation_dir =
            wardian_core::paths::agent_conversation_dir("agent-1", &conversation_id)
                .expect("conversation dir");
        let records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &conversation_dir.join("conversation.jsonl"),
            )
            .expect("conversation records");
        let events: Vec<AgentChatEvent> =
            wardian_core::conversations::read_jsonl_records(&conversation_dir.join("events.jsonl"))
                .expect("event records");

        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].text.as_deref(),
            Some("From Source: review this change.")
        );
        assert_eq!(
            records[0].speaker_type,
            Some(wardian_core::conversations::ConversationSpeakerType::Agent)
        );
        assert_eq!(records[1].text.as_deref(), Some("I reviewed it."));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "headless:interaction-1:user");
        assert_eq!(events[1].id, "headless:interaction-1:assistant");
        assert_eq!(
            events[1].metadata["provider_session_id"].as_str(),
            Some("provider-session-1")
        );
    }

    #[tokio::test]
    async fn message_delivery_queues_when_current_conversation_is_leased() {
        let _home = TestWardianHome::new();

        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            let mut config = agent.config.lock().unwrap();
            config.provider = "mock".to_string();
            config.resume_session = Some("resume-1".to_string());
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;
        wardian_core::conversation_lease::acquire_lease(
            wardian_core::conversation_lease::ConversationLease {
                agent_id: "agent-1".to_string(),
                provider: "mock".to_string(),
                resume_session: "resume-1".to_string(),
                owner_kind: "workflow_run".to_string(),
                owner_id: "wf/run-1/node-1".to_string(),
                acquisition_id: "test-acquisition-1".to_string(),
                owner_node_id: Some("node-1".to_string()),
                mode: "background_resume".to_string(),
                started_at: "2026-06-01T00:00:00Z".to_string(),
                heartbeat_at: "2026-06-01T00:00:00Z".to_string(),
                expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
            },
            &chrono::Utc::now().to_rfc3339(),
        )
        .expect("lease");

        let delivery = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "hello",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
            false,
        )
        .await
        .unwrap();

        assert_eq!(delivery[0].delivery_state, "queued");
        assert_eq!(delivery[0].runtime_state, "conversation_leased");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn fresh_headless_message_lease_marks_the_off_agent_headless() {
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").expect("agent");
            *agent.current_status.lock().expect("status") = "Off".to_string();
        }
        let info = delivery_target_infos(&state, &["agent-1".to_string()])
            .await
            .expect("delivery info")
            .remove(0);

        let lease = acquire_headless_message_lease(&info, "interaction-1").expect("lease");

        assert!(lease.resume_session.is_empty());
        assert_eq!(lease.mode, "background_fresh");
        let snapshot = {
            let agents = state.agents.lock().await;
            snapshot_agent(agents.get("agent-1").expect("agent"))
        };
        assert_eq!(snapshot.status, "headless");
        wardian_core::conversation_lease::release_owner_persisted(
            &lease.owner_kind,
            &lease.owner_id,
        )
        .expect("release lease");
    }

    #[tokio::test]
    async fn offline_message_does_not_start_while_worktree_mutation_is_active() {
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").expect("agent");
            *agent.current_status.lock().expect("status") = "Off".to_string();
        }
        let _mutation =
            wardian_core::workflow_execution_lock::try_acquire_worktree_mutation_guard()
                .expect("worktree mutation lock")
                .expect("exclusive worktree mutation lock");

        let error = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "do not start a provider",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
            false,
        )
        .await
        .expect_err("active deletion must block direct headless delivery");

        assert_eq!(error.code(), "request_failed");
        let detail = error
            .details()
            .and_then(|details| details.get("delivery"))
            .and_then(serde_json::Value::as_array)
            .and_then(|delivery| delivery.first())
            .expect("failed delivery detail");
        assert_eq!(detail["delivery_state"], "failed");
        assert_eq!(detail["error"]["code"], "headless_execution_blocked");
        assert!(wardian_core::conversation_lease::load_leases().is_empty());

        let status = {
            let agents = state.agents.lock().await;
            snapshot_agent(agents.get("agent-1").expect("agent")).status
        };
        assert_eq!(status, "off");
    }

    #[tokio::test]
    async fn headless_status_observations_follow_the_lease_lifecycle() {
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").expect("agent");
            *agent.current_status.lock().expect("status") = "Off".to_string();
        }
        let cursor = {
            let agents = state.agents.lock().await;
            let cursor = agents
                .get("agent-1")
                .expect("agent")
                .watch_state
                .lock()
                .expect("watch state")
                .latest_cursor();
            cursor
        };
        let info = delivery_target_infos(&state, &["agent-1".to_string()])
            .await
            .expect("delivery info")
            .remove(0);

        let lease = acquire_headless_message_lease(&info, "interaction-1").expect("lease");
        record_headless_status_observation(None, &state, &info).await;
        wardian_core::conversation_lease::release_owner_persisted(
            &lease.owner_kind,
            &lease.owner_id,
        )
        .expect("release lease");
        record_headless_status_observation(None, &state, &info).await;

        let events = {
            let agents = state.agents.lock().await;
            let events = agents
                .get("agent-1")
                .expect("agent")
                .watch_state
                .lock()
                .expect("watch state")
                .snapshot_since(Some(&cursor), None)
                .expect("watch snapshot")
                .events;
            events
        };
        let statuses: Vec<_> = events
            .iter()
            .filter(|event| event.kind == "status")
            .filter_map(|event| event.payload["status"].as_str())
            .collect();

        assert_eq!(statuses, ["headless", "off"]);
    }

    #[tokio::test]
    async fn provider_turn_start_receipt_requires_an_event_after_the_submit_cursor() {
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;

        crate::manager::record_agent_turn_started_for_watch(&state, "agent-1").await;
        let cursor = provider_turn_start_cursor(&state, "agent-1")
            .await
            .expect("cursor");
        let wait = wait_for_provider_turn_started_after_submit(&state, "agent-1", &cursor);
        tokio::pin!(wait);

        tokio::select! {
            result = &mut wait => panic!("pre-submit event must not satisfy receipt: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }

        crate::manager::record_agent_turn_started_for_watch(&state, "agent-1").await;
        wait.await.expect("post-submit provider turn receipt");
    }

    #[tokio::test]
    async fn headless_message_queues_while_a_resume_lifecycle_gate_is_held() {
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").expect("agent");
            *agent.current_status.lock().expect("status") = "Off".to_string();
        }

        let lifecycle_guard = state.lock_agent_lifecycle("agent-1").await;
        let delivery = deliver_message_to_target(
            None,
            &state,
            "CoderOne",
            "do not overlap a resume",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
            false,
        )
        .await
        .expect("queue while lifecycle transition owns the gate");

        assert_eq!(delivery[0].delivery_state, "queued");
        assert_eq!(delivery[0].runtime_state, "conversation_leased");
        drop(lifecycle_guard);
    }

    #[tokio::test]
    async fn late_headless_completion_skips_a_replaced_agent_incarnation() {
        let _home = TestWardianHome::new();
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: wardian_core::conversations::ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("save shell settings");
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").expect("agent");
            agent.config.lock().expect("config").conversation_logging =
                wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        }
        let stale_info = delivery_target_infos(&state, &["agent-1".to_string()])
            .await
            .expect("delivery info")
            .remove(0);

        // Clear and resume replace the ActiveAgent entry with a new runtime
        // incarnation. A late completion from the old process must not appear
        // in the successor's watch stream or conversation archive.
        state.agents.lock().await.insert(
            "agent-1".to_string(),
            test_agent("agent-1", "CoderOne", "Coder"),
        );

        record_headless_message_response(
            &state,
            &stale_info,
            "old-interaction",
            "stale provider response",
        )
        .await;
        record_headless_message_exchange(
            &state,
            &stale_info,
            "old-interaction",
            "stale prompt",
            "stale provider response",
            None,
        )
        .await;

        let snapshot = {
            let agents = state.agents.lock().await;
            let snapshot = agents
                .get("agent-1")
                .expect("replacement agent")
                .watch_state
                .lock()
                .expect("watch state")
                .snapshot_since(None, None)
                .expect("watch snapshot");
            snapshot
        };
        assert!(snapshot.output.text.is_empty());
        assert!(snapshot.transcript.messages.is_empty());
        assert!(state
            .conversation_archive
            .active_conversation_id_for_test("agent-1")
            .is_none());
    }

    #[tokio::test]
    async fn late_headless_completion_skips_an_agent_removed_by_kill() {
        let _home = TestWardianHome::new();
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: wardian_core::conversations::ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("save shell settings");
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").expect("agent");
            agent.config.lock().expect("config").conversation_logging =
                wardian_core::conversations::AgentConversationLoggingSetting::Enabled;
        }
        let stale_info = delivery_target_infos(&state, &["agent-1".to_string()])
            .await
            .expect("delivery info")
            .remove(0);

        state.agents.lock().await.remove("agent-1");
        record_headless_message_response(
            &state,
            &stale_info,
            "removed-interaction",
            "stale provider response",
        )
        .await;
        record_headless_message_exchange(
            &state,
            &stale_info,
            "removed-interaction",
            "stale prompt",
            "stale provider response",
            None,
        )
        .await;

        assert!(state
            .conversation_archive
            .active_conversation_id_for_test("agent-1")
            .is_none());
    }

    #[tokio::test]
    async fn concurrent_offline_messages_queue_instead_of_failing_on_a_lease_race() {
        if !node_available() {
            return;
        }
        let _home = TestWardianHome::new();
        let _scenario = ScopedEnvVar::set("WARDIAN_MOCK_SCENARIO", "headless_delayed");
        let _delay = ScopedEnvVar::set("WARDIAN_MOCK_DELAY_MS", "250");
        let workspace = tempfile::tempdir().expect("workspace");
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").expect("agent");
            let mut config = agent.config.lock().expect("config");
            config.provider = "mock".to_string();
            config.folder = workspace.path().to_string_lossy().to_string();
            *agent.current_status.lock().expect("status") = "Off".to_string();
        }

        let (first, second) = tokio::join!(
            deliver_message_to_target(
                None,
                &state,
                "CoderOne",
                "first offline message",
                None,
                MessageInputMode::Message,
                QueuePolicy::QueueIfBusy,
                None,
                None,
                false,
            ),
            deliver_message_to_target(
                None,
                &state,
                "CoderOne",
                "second offline message",
                None,
                MessageInputMode::Message,
                QueuePolicy::QueueIfBusy,
                None,
                None,
                false,
            ),
        );
        let delivery = [
            first.expect("first delivery").remove(0),
            second.expect("second delivery").remove(0),
        ];

        assert_eq!(
            delivery
                .iter()
                .filter(|detail| detail.delivery_state == "provider_applied")
                .count(),
            1
        );
        assert_eq!(
            delivery
                .iter()
                .filter(|detail| {
                    detail.delivery_state == "queued"
                        && detail.runtime_state == "conversation_leased"
                })
                .count(),
            1
        );
        assert!(delivery
            .iter()
            .all(|detail| detail.delivery_state != "failed"));
    }

    #[tokio::test]
    async fn successful_headless_target_archives_before_a_mixed_send_reports_failure() {
        if !node_available() {
            return;
        }
        let _home = TestWardianHome::new();
        let _scenario = ScopedEnvVar::set("WARDIAN_MOCK_SCENARIO", "headless");
        let workspace = tempfile::tempdir().expect("workspace");
        crate::utils::save_shell_settings(&crate::utils::ShellSettings {
            conversation_logging: wardian_core::conversations::ConversationLoggingSetting::Enabled,
            ..Default::default()
        })
        .expect("save shell settings");
        let state = AppState::new();
        insert_test_agent(&state, "agent-success", "Success", "Coder").await;
        insert_test_agent(&state, "agent-failure", "Failure", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let success = agents.get("agent-success").expect("success agent");
            let mut success_config = success.config.lock().expect("success config");
            success_config.provider = "mock".to_string();
            success_config.folder = workspace.path().to_string_lossy().to_string();
            *success.current_status.lock().expect("success status") = "Off".to_string();

            let failure = agents.get("agent-failure").expect("failure agent");
            failure.config.lock().expect("failure config").provider = "not-a-provider".to_string();
            *failure.current_status.lock().expect("failure status") = "Off".to_string();
        }

        let error = deliver_message_to_target(
            None,
            &state,
            "all",
            "record this before aggregate failure",
            None,
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
            None,
            true,
        )
        .await
        .expect_err("one target cannot launch");

        assert_eq!(error.code, "request_failed");
        let conversation_id = state
            .conversation_archive
            .active_conversation_id_for_test("agent-success")
            .expect("successful target archive");
        let records: Vec<wardian_core::conversations::ConversationNarrativeRecord> =
            wardian_core::conversations::read_jsonl_records(
                &wardian_core::paths::agent_conversation_dir("agent-success", &conversation_id)
                    .expect("conversation dir")
                    .join("conversation.jsonl"),
            )
            .expect("conversation records");
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].text.as_deref(),
            Some("record this before aggregate failure")
        );
        assert_eq!(
            records[1].text.as_deref(),
            Some("Mock headless execution completed successfully.")
        );
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
    fn delivery_route_runs_offline_message_headlessly_when_queue_if_busy() {
        for status in ["off", "error"] {
            let route = decide_delivery_route(
                status,
                MessageInputMode::Message,
                QueuePolicy::QueueIfBusy,
                None,
            );

            assert_eq!(route, DeliveryRoute::Headless, "status={status}");
        }
    }

    #[test]
    fn headless_delivery_timeout_is_bounded_for_control_requests() {
        assert_eq!(
            bounded_headless_delivery_timeout(None),
            crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT
        );
        assert_eq!(
            bounded_headless_delivery_timeout(Some(0)),
            Duration::from_secs(1)
        );
        assert_eq!(
            bounded_headless_delivery_timeout(Some(20 * 60 * 1000)),
            MAX_HEADLESS_DELIVERY_TIMEOUT
        );
    }

    #[test]
    fn delivery_route_keeps_off_provider_command_in_the_mailbox() {
        let route = decide_delivery_route(
            "off",
            MessageInputMode::Command,
            QueuePolicy::QueueIfBusy,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Mailbox {
                runtime_state: "queued_not_live"
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

        assert_eq!(route, DeliveryRoute::Live);
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
        let req = build_spawn_agent_request(
            "codex".to_string(),
            "Reviewer".to_string(),
            None,
            None,
            None,
            None,
        )
        .expect("spawn request");

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
    fn spawn_request_applies_model_and_effort_to_the_provider_config() {
        let req = build_spawn_agent_request(
            "codex".to_string(),
            "Reviewer".to_string(),
            None,
            None,
            Some("gpt-5.6-sol".to_string()),
            Some("high".to_string()),
        )
        .expect("spawn request");
        let config = req.config_override.expect("config override");

        assert_eq!(config.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            config.codex_config().reasoning_effort.as_deref(),
            Some("high")
        );
    }

    #[test]
    fn model_and_effort_updates_require_a_restart_for_running_agents() {
        assert!(agent_update_requires_restart(&["model".to_string()], false));
        assert!(agent_update_requires_restart(
            &["reasoning_effort".to_string()],
            false
        ));
        assert!(!agent_update_requires_restart(&["model".to_string()], true));
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
    async fn broadcast_targets_scope_to_sender_neighbors() {
        let home = TestWardianHome::new();
        let mut topology = wardian_core::topology::Topology::default();
        topology.add_edge("sender-1", "peer-1", "2026-07-02T00:00:00Z");
        wardian_core::topology::save_topology(home.path(), &topology).unwrap();

        let state = AppState::new();
        insert_test_agent(&state, "sender-1", "SenderOne", "Coder").await;
        insert_test_agent(&state, "peer-1", "PeerOne", "Reviewer").await;
        insert_test_agent(&state, "stranger-1", "StrangerOne", "Coder").await;

        // Neighbors-scoped send to "all": only peer-1 (in neighbors)
        let neighbors = resolve_send_targets_scoped(&state, "all", Some("sender-1"), false).await;
        let mut neighbors = neighbors;
        neighbors.sort();
        assert_eq!(neighbors, vec!["peer-1".to_string()]);

        // Globally-scoped send to "all": all agents (including sender)
        let global = resolve_send_targets_scoped(&state, "all", Some("sender-1"), true).await;
        let mut global = global;
        global.sort();
        assert_eq!(
            global,
            vec![
                "peer-1".to_string(),
                "sender-1".to_string(),
                "stranger-1".to_string()
            ]
        );

        // No sender (human origin): same as scope_all=true (all agents)
        let human_send = resolve_send_targets_scoped(&state, "all", None, false).await;
        let mut human_send = human_send;
        human_send.sort();
        assert_eq!(
            human_send,
            vec![
                "peer-1".to_string(),
                "sender-1".to_string(),
                "stranger-1".to_string()
            ]
        );

        // Exact UUID targeting always works (soft boundary)
        let exact_uuid =
            resolve_send_targets_scoped(&state, "stranger-1", Some("sender-1"), false).await;
        assert_eq!(exact_uuid, vec!["stranger-1".to_string()]);
    }

    #[tokio::test]
    async fn message_delivery_writes_terminal_bytes_to_matched_agent() {
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "source-1", "PlannerOne", "Planner").await;
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "source-1", "PlannerOne", "Planner").await;
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "source-1", "PlannerOne", "Planner").await;
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Action Needed".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            agent.config.lock().unwrap().provider = "codex".to_string();
            *agent.current_status.lock().unwrap() = "Action Needed".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
        assert_eq!(delivery[0].runtime_state, "live_pty_available");
        assert_eq!(delivery[0].delivery_state, "approval_submitted");
        assert_eq!(
            delivery[0].delivery_phase.as_deref(),
            Some("approval_key_sent")
        );
    }

    #[tokio::test]
    async fn approval_send_failure_is_not_retry_safe_after_the_terminal_boundary() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);

        let error = submit_approval_action_via_sender(&tx, "codex", &ApprovalAction::Accept)
            .await
            .expect_err("closed input channel");

        assert_eq!(error.phase, "approval_send_failed");
        assert!(!error.retry_safe);
    }

    #[tokio::test]
    async fn mailbox_drain_submits_next_pending_message_when_target_is_idle() {
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        assert_eq!(drained.delivery_state, "submit_sent_unconfirmed");
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
            assert!(snapshot.events.iter().any(|event| {
                event.kind == "delivery"
                    && event.payload["runtime_state"] == "mailbox_drain"
                    && event.payload["delivery_state"] == "submit_sent_unconfirmed"
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
        )
        .await
        .unwrap();

        assert_eq!(delivery[0].runtime_state, "provider_input_not_ready");
        assert_eq!(delivery[0].delivery_state, "queued");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn claude_idle_status_allows_live_delivery_despite_stale_readiness() {
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
        )
        .await
        .unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"queued work".to_vec());
        assert_eq!(rx.recv().await.unwrap(), b"\r".to_vec());
        assert_eq!(delivery[0].runtime_state, "live_pty_available");
    }

    #[tokio::test]
    async fn mailbox_drain_can_complete_booting_provider_from_prompt_evidence() {
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        assert_eq!(drained.delivery_state, "submit_sent_unconfirmed");
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        assert_eq!(drained.delivery_state, "submit_sent_unconfirmed");
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drained message");

        assert_eq!(drained.runtime_state, "mailbox_drain");
        assert_eq!(drained.delivery_state, "submit_sent_unconfirmed");
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drained message");

        assert_eq!(drained.runtime_state, "mailbox_drain");
        assert_eq!(drained.delivery_state, "submit_sent_unconfirmed");
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
            false,
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

        assert_eq!(drained.delivery_state, "submit_sent_unconfirmed");
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
    async fn mailbox_drain_waits_while_current_conversation_is_leased() {
        let _home = TestWardianHome::new();

        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            let mut config = agent.config.lock().unwrap();
            config.resume_session = Some("resume-1".to_string());
            *agent.current_status.lock().unwrap() = "Processing".to_string();
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
            false,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        wardian_core::conversation_lease::acquire_lease(
            wardian_core::conversation_lease::ConversationLease {
                agent_id: "agent-1".to_string(),
                provider: "mock".to_string(),
                resume_session: "resume-1".to_string(),
                owner_kind: "workflow_run".to_string(),
                owner_id: "wf/run-1/node-1".to_string(),
                acquisition_id: "test-acquisition-2".to_string(),
                owner_node_id: Some("node-1".to_string()),
                mode: "background_resume".to_string(),
                started_at: "2026-06-01T00:00:00Z".to_string(),
                heartbeat_at: "2026-06-01T00:00:00Z".to_string(),
                expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
            },
            &chrono::Utc::now().to_rfc3339(),
        )
        .expect("lease");

        let drained = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap();

        assert!(drained.is_none());
        let records = state.mailbox.lock().await.list_for_target("agent-1");
        assert_eq!(records[0].id, message_id);
        assert_eq!(
            records[0].status,
            crate::state::MailboxMessageStatus::Pending
        );
    }

    #[tokio::test]
    async fn mailbox_drain_missing_sender_leaves_message_pending_for_next_observation() {
        let _home = TestWardianHome::new();
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
            false,
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
    async fn mailbox_drain_stops_when_a_ready_agent_has_no_input_channel() {
        let _home = TestWardianHome::new();
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
            false,
        )
        .await
        .unwrap();
        let message_id = queued[0].message_id.clone().unwrap();
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        state
            .interactions
            .record_provider_input_state(
                "agent-1",
                1,
                ProviderInputReadiness::Ready,
                Some(ProviderReadyEvidence::ProviderEvent),
            )
            .await;

        let attempt = drain_next_mailbox_message_for_idle_agent(None, &state, "agent-1")
            .await
            .unwrap()
            .expect("drain attempt");

        assert_eq!(attempt.delivery_state, "failed");
        assert_eq!(attempt.message_id.as_deref(), Some(message_id.as_str()));
        assert_eq!(
            attempt.error.as_ref().map(|error| error.code.as_str()),
            Some("no_input_channel")
        );
        assert!(attempt
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("delivery stopped"));
        let records = state.mailbox.lock().await.list_for_target("agent-1");
        assert_eq!(
            records[0].status,
            crate::state::MailboxMessageStatus::Failed
        );
    }

    #[tokio::test]
    async fn mailbox_drain_submit_key_failure_marks_failed_without_retry() {
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
    async fn mailbox_drain_payload_send_failure_fails_without_replay() {
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
            crate::state::MailboxMessageStatus::Failed
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
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "source-1", "PlannerOne", "Planner").await;
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        {
            let agents = state.agents.lock().await;
            let agent = agents.get("agent-1").unwrap();
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
            false,
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
        let _home = TestWardianHome::new();

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
            false,
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
        let _home = TestWardianHome::new();
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
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        let _home = TestWardianHome::new();
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        install_test_terminal_runtime(&state, "agent-1", tx).await;

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
            false,
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
        assert!(delivery.prompt.contains("execute this command"));
        assert!(delivery.prompt.contains("Do not print the command"));
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
            None,
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

    #[test]
    fn multi_ask_delivery_failure_keeps_target_scoped_evidence() {
        let result = ask_target_failure(
            "missing-reviewer",
            AskTargetOutcome::DeliveryFailed,
            "not_found",
            "agent not found: missing-reviewer".to_string(),
        );

        assert_eq!(result.target, "missing-reviewer");
        assert_eq!(result.outcome, AskTargetOutcome::DeliveryFailed);
        assert!(result.request_id.is_none());
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.code.as_str()),
            Some("not_found")
        );
    }

    #[tokio::test]
    async fn multi_ask_timeout_records_a_terminal_reply() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        let request_id = new_ask_request_id();
        state
            .interactions
            .create_task_with_id(
                request_id.clone(),
                None,
                "agent-1".to_string(),
                InteractionBodyRef::Inline {
                    body: "review this".to_string(),
                },
            )
            .await;

        let reply = fail_structured_ask_request(
            &state,
            &request_id,
            "agent-1",
            "structured reply timed out",
            None,
        )
        .await
        .expect("timeout should record a terminal reply");

        assert_eq!(reply.status, wardian_core::control::ReplyStatus::Failed);
        assert_eq!(reply.body, "structured reply timed out");
        let late = submit_structured_reply(
            &state,
            &request_id,
            wardian_core::control::ReplyStatus::Done,
            "late",
            Some(&wardian_core::control::MessageOrigin::WardianAgent {
                session_id: "agent-1".to_string(),
            }),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(late.code(), "duplicate_reply");
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
            None,
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
            submit_structured_reply(
                &state,
                &request_id,
                status.clone(),
                "cannot continue",
                None,
                None,
            )
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
    fn conditional_watch_ignores_retained_idle_until_a_new_observation_arrives() {
        let mut state = crate::state::AgentWatchState::new("agent-1".to_string(), 16, 1024);
        state.push_event("status", serde_json::json!({"status":"idle"}));

        let since = watch_start_cursor(&state, None, true).expect("conditional baseline");
        let stale_snapshot = state.snapshot_since(Some(&since), Some(1024)).unwrap();
        assert!(!watch_condition_matches(
            &WatchCondition::Status("idle".to_string()),
            &stale_snapshot,
            None,
        ));

        state.push_event("status", serde_json::json!({"status":"idle"}));
        let fresh_snapshot = state.snapshot_since(Some(&since), Some(1024)).unwrap();
        assert!(watch_condition_matches(
            &WatchCondition::Status("idle".to_string()),
            &fresh_snapshot,
            None,
        ));
    }

    #[test]
    fn conditional_watch_honors_an_explicit_historical_cursor() {
        let mut state = crate::state::AgentWatchState::new("agent-1".to_string(), 16, 1024);
        let historical_cursor = state.latest_cursor();
        state.push_event("status", serde_json::json!({"status":"idle"}));

        let since = watch_start_cursor(&state, Some(historical_cursor), true);
        let snapshot = state.snapshot_since(since.as_deref(), Some(1024)).unwrap();
        assert!(watch_condition_matches(
            &WatchCondition::Status("idle".to_string()),
            &snapshot,
            None,
        ));
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

    #[test]
    fn snapshot_agent_reports_headless_while_its_saved_conversation_is_leased() {
        let _home = TestWardianHome::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder");
        {
            let mut config = agent.config.lock().unwrap();
            config.resume_session = Some("provider-session-1".to_string());
            config.is_off = true;
            *agent.current_status.lock().unwrap() = "Off".to_string();
        }
        let now = chrono::Utc::now();
        wardian_core::conversation_lease::acquire_lease(
            wardian_core::conversation_lease::ConversationLease {
                agent_id: "agent-1".to_string(),
                provider: "mock".to_string(),
                resume_session: "provider-session-1".to_string(),
                owner_kind: "message_delivery".to_string(),
                owner_id: "int-1".to_string(),
                acquisition_id: "test-acquisition-3".to_string(),
                owner_node_id: None,
                mode: "background_resume".to_string(),
                started_at: now.to_rfc3339(),
                heartbeat_at: now.to_rfc3339(),
                expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
            },
            &now.to_rfc3339(),
        )
        .expect("lease");

        let snapshot = snapshot_agent(&agent);

        assert_eq!(snapshot.status, "headless");
    }

    #[test]
    fn snapshot_agent_keeps_a_live_agent_status_while_a_fresh_background_run_is_leased() {
        let _home = TestWardianHome::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder");
        {
            let mut config = agent.config.lock().unwrap();
            config.is_off = false;
            *agent.current_status.lock().unwrap() = "Idle".to_string();
        }
        let now = chrono::Utc::now();
        wardian_core::conversation_lease::acquire_lease(
            wardian_core::conversation_lease::ConversationLease {
                agent_id: "agent-1".to_string(),
                provider: "mock".to_string(),
                resume_session: String::new(),
                owner_kind: "workflow_run".to_string(),
                owner_id: "workflow/fresh".to_string(),
                acquisition_id: "test-acquisition-4".to_string(),
                owner_node_id: Some("plan".to_string()),
                mode: "background_fresh".to_string(),
                started_at: now.to_rfc3339(),
                heartbeat_at: now.to_rfc3339(),
                expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
            },
            &now.to_rfc3339(),
        )
        .expect("lease");

        let snapshot = snapshot_agent(&agent);

        assert_eq!(snapshot.status, "idle");
    }
}
