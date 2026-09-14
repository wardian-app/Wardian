mod pi_startup;
use pi_startup::pi_output_has_startup_ready_prompt;

pub(crate) mod codex_menu_status;
pub(crate) mod startup_readiness;
use startup_readiness::record_provider_ready_evidence;
pub(crate) use startup_readiness::{
    provider_output_has_startup_ready_prompt, provider_output_requires_startup_action,
};

use crate::manager;
mod agent_messaging;
pub(crate) use agent_messaging::message_with_structured_reply_instruction;
mod codex_background;
mod headless_delivery;
use crate::remote::operations::inbox_list_control as list_inbox_control;
use crate::state::conversation_archive::{
    effective_conversation_logging, ConversationArchiveContext,
};
use crate::state::AppState;
use crate::utils::strip_ansi_controls;
#[cfg(test)]
use agent_messaging::bounded_headless_delivery_timeout;
use agent_messaging::HeadlessMessageDeliveryRequest;
use headless_delivery::deliver_headless_message;
use std::{
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{
    AgentDoctorResponse, AgentListResponse, AgentResponse, AgentUpdateResponse, AgentWatchResponse,
    AgentWorktreeListResponse, AgentWorktreeMutationResponse, AgentWorktreeSummary, ApprovalAction,
    CodexPluginDiagnostic, ControlRequest, ConversationListResponse, ConversationShowResponse,
    DeliveryDetail, DeliveryErrorDetail, DeliveryTransportKind, InboxNotificationKind,
    InboxNotificationPayload, InboxNotificationResponse, InteractionBodyRef, InteractionStatus,
    MessageInputMode, MessageOrigin, OkResponse, ProviderInputReadiness, ProviderReadyEvidence,
    QueuePolicy, WatchAgentSnapshot, WatchDeliverySnapshot,
};
use wardian_core::conversations::ConversationLoggingSetting;
use wardian_core::identity::{normalize_status, AgentIdentity, StatusSource};
use wardian_core::models::{AgentChatEvent, AgentChatEventKind, AgentChatRole};
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
    manager::try_save_state_snapshot_unlocked(&snapshot)
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
        ControlRequest::AgentMessaging { request, origin } => {
            agent_messaging::handle(app, request, origin).await
        }
        ControlRequest::AgentList => {
            let response = AgentListResponse::new(live_agent_snapshots(app).await);
            ok_json(&response)
        }

        ControlRequest::BrowserOpen {
            url,
            agent,
            workspace,
            width,
            height,
            detached,
            blank,
        } => ok_json(
            &crate::commands::browser::open_session(
                app, url, agent, workspace, width, height, detached, blank,
            )
            .await
            .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserList => ok_json(&crate::commands::browser::list_sessions(app).await),

        ControlRequest::BrowserClose { target } => {
            let browser_id = crate::commands::browser::close_session(app, &target)
                .await
                .map_err(browser_control_error)?;
            ok_json(&serde_json::json!({ "browser_id": browser_id }))
        }

        ControlRequest::BrowserNavigate { target, action } => ok_json(
            &crate::commands::browser::navigate_session(app, &target, &action, None)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserGet {
            target,
            field,
            selector,
        } => ok_json(
            &crate::commands::browser::get_field(app, &target, &field, selector.as_deref())
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserWait {
            target,
            load_state,
            selector,
            text,
            url_contains,
            function,
            timeout_ms,
        } => {
            let condition = crate::commands::browser::wait_condition_from_parts(
                load_state.as_deref(),
                selector.as_deref(),
                text.as_deref(),
                url_contains.as_deref(),
                function.as_deref(),
            )
            .map_err(browser_control_error)?;
            ok_json(
                &crate::commands::browser::wait_for(app, &target, &condition, timeout_ms)
                    .await
                    .map_err(browser_control_error)?,
            )
        }

        ControlRequest::BrowserSnapshot {
            target,
            interactive,
        } => ok_json(
            &crate::commands::browser::snapshot_session(app, &target, interactive)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserAct {
            target,
            element_ref,
            action,
            value,
            snapshot_after,
        } => {
            let parsed =
                crate::commands::browser::element_action_from_parts(&action, value.as_deref())
                    .map_err(browser_control_error)?;
            ok_json(
                &crate::commands::browser::act_on_session(
                    app,
                    &target,
                    &element_ref,
                    &parsed,
                    snapshot_after,
                )
                .await
                .map_err(browser_control_error)?,
            )
        }

        ControlRequest::BrowserScreenshot {
            target,
            path,
            full_page,
        } => ok_json(
            &crate::commands::browser::screenshot_session(app, &target, &path, full_page)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserViewport {
            target,
            width,
            height,
            reset,
        } => ok_json(
            &crate::commands::browser::set_session_viewport(
                app, &target, width, height, reset, None,
            )
            .await
            .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserEval { target, expression } => ok_json(
            &crate::commands::browser::eval_in_session(app, &target, &expression)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserConsole {
            target,
            level,
            clear,
        } => ok_json(
            &crate::commands::browser::console_for_session(app, &target, level.as_deref(), clear)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserNetwork { target, action } => ok_json(
            &crate::commands::browser::network_for_session(app, &target, &action)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserCookies { target, action } => ok_json(
            &crate::commands::browser::cookies_for_session(app, &target, &action)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserStorage {
            target,
            area,
            action,
        } => ok_json(
            &crate::commands::browser::storage_for_session(app, &target, area, &action)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::BrowserDownloads { target, clear } => ok_json(
            &crate::commands::browser::downloads_for_session(app, &target, clear)
                .await
                .map_err(browser_control_error)?,
        ),

        ControlRequest::AgentDelete {
            target,
            confirm_name,
            force,
        } => {
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            let state = app.state::<AppState>();
            crate::commands::agent::delete_agent(uuid, confirm_name, force, state, app.clone())
                .await
                .map_err(ControlError::bad_request)?;
            ok_json(&OkResponse::new())
        }

        ControlRequest::AgentRename { target, name } => {
            let uuid = resolve_target_uuid(app, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            crate::commands::agent::rename_agent(
                uuid.clone(),
                name,
                app.state::<AppState>(),
                app.clone(),
            )
            .await
            .map_err(ControlError::bad_request)?;
            let identity = live_agent_identity(app, &uuid).await?;
            ok_json(&AgentUpdateResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                ok: true,
                agent: identity,
                updated_fields: vec!["name".to_string()],
                restart_required: false,
            })
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
            let provider = crate::commands::agent::canonical_agent_provider_name(&provider)
                .map_err(ControlError::bad_request)?;
            let class = crate::commands::agent::canonical_agent_class_name(&class)
                .map_err(ControlError::not_found)?;
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
                if wardian_core::classes::find_class(&classes, class).is_none() {
                    return Err(ControlError::not_found(format!(
                        "agent class not found: {class}"
                    )));
                }
            }

            let state = app.state::<AppState>();
            let _roster_barrier =
                wardian_core::agent_replacement::acquire_agent_roster_barrier(true)
                    .map_err(ControlError::request_failed)?
                    .ok_or_else(|| {
                        ControlError::request_failed("agent roster barrier is unavailable")
                    })?;
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
            if let Err(error) = manager::try_save_state_snapshot_unlocked(&outcome.state_snapshot) {
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
        request @ ControlRequest::InboxList { .. } => list_inbox_control(app, request).await,
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

        ControlRequest::TopologyLink {
            a,
            b,
            caller_session_id,
        } => {
            topology_mutation(
                app,
                wardian_core::topology::TopologyOperation::Link,
                a,
                b,
                caller_session_id,
            )
            .await
        }
        ControlRequest::TopologyUnlink {
            a,
            b,
            caller_session_id,
        } => {
            topology_mutation(
                app,
                wardian_core::topology::TopologyOperation::Unlink,
                a,
                b,
                caller_session_id,
            )
            .await
        }
        ControlRequest::TopologyIgnore {
            a,
            b,
            caller_session_id,
        } => {
            topology_mutation(
                app,
                wardian_core::topology::TopologyOperation::Ignore,
                a,
                b,
                caller_session_id,
            )
            .await
        }
        ControlRequest::TopologyUnignore {
            a,
            b,
            caller_session_id,
        } => {
            topology_mutation(
                app,
                wardian_core::topology::TopologyOperation::Unignore,
                a,
                b,
                caller_session_id,
            )
            .await
        }

        ControlRequest::WatchlistsChanged => {
            let _ = app.emit("watchlists-updated", ());
            ok_json(&OkResponse::new())
        }

        request @ ControlRequest::AutomationRun { .. } => {
            handle_automation_run_control(app, automation_run_control_launch(request)?).await
        }

        ControlRequest::NotifyCreate {
            notification,
            origin,
        } => {
            let MessageOrigin::WardianAgent { session_id } = origin;
            validate_inbox_notification(&notification)?;
            let state = app.state::<AppState>();
            let _origin_lifecycle_guard = state.lock_agent_lifecycle(&session_id).await;
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

        ControlRequest::DeliveryGet {
            interaction_id,
            evidence_limit,
        } => {
            let state = app.state::<AppState>();
            let record = state
                .native_delivery
                .get(&interaction_id)
                .map_err(native_broker_control_error)?;
            let evidence = state
                .native_delivery
                .evidence(&interaction_id, evidence_limit.unwrap_or(100).min(500))
                .map_err(native_broker_control_error)?;
            ok_json(&wardian_core::control::NativeDeliveryInspectResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                record,
                evidence,
            })
        }

        ControlRequest::DeliveryCancel { interaction_id } => {
            let state = app.state::<AppState>();
            let record = state
                .native_delivery
                .cancel(&interaction_id)
                .await
                .map_err(native_broker_control_error)?;
            let evidence = state
                .native_delivery
                .evidence(&interaction_id, 100)
                .map_err(native_broker_control_error)?;
            ok_json(&wardian_core::control::NativeDeliveryInspectResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                record,
                evidence,
            })
        }

        ControlRequest::DeliveryWithdraw { interaction_id } => {
            let state = app.state::<AppState>();
            let record = state
                .native_delivery
                .withdraw(&interaction_id)
                .await
                .map_err(native_broker_control_error)?;
            let _ = state
                .interactions
                .update_message_status_durable(&interaction_id, InteractionStatus::Failed)
                .await;
            let evidence = state
                .native_delivery
                .evidence(&interaction_id, 100)
                .map_err(native_broker_control_error)?;
            ok_json(&wardian_core::control::NativeDeliveryInspectResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                record,
                evidence,
            })
        }

        ControlRequest::DeliveryCapabilities { target } => {
            let state = app.state::<AppState>();
            let target_agent_id = resolve_target_uuid_in_state(&state, &target)
                .await
                .ok_or_else(|| ControlError::not_found(format!("agent not found: {target}")))?;
            let info = delivery_target_info(&state, &target_agent_id).await?;
            let protocol = crate::delivery::native_session::NativeProviderProtocol::for_provider(
                &info.provider,
            )
            .ok_or_else(|| {
                ControlError::not_supported(format!(
                    "{} has no Wardian native transport",
                    info.provider
                ))
            })?;
            let binding = wardian_core::db::latest_native_session_binding(&target_agent_id)
                .map_err(|error| ControlError::request_failed(error.to_string()))?;
            let candidate_capabilities = protocol.capabilities("unverified");
            ok_json(&wardian_core::control::NativeDeliveryCapabilitiesResponse {
                schema: wardian_core::control::CONTROL_SCHEMA,
                target_agent_id,
                broker_queue_withdrawal: true,
                broker_queue_replacement: true,
                native_negotiated: binding.is_some(),
                capabilities: binding
                    .as_ref()
                    .map(|binding| binding.capabilities.clone())
                    .unwrap_or_else(|| {
                        wardian_core::native_transport::NativeTransportCapabilities::degraded(
                            &info.provider,
                            "headless_fallback",
                        )
                    }),
                candidate_capabilities,
                binding,
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
struct AutomationRunControlLaunch {
    path: String,
    provider: Option<String>,
    workspace: Option<String>,
    input: Option<serde_json::Value>,
    bindings: Option<std::collections::HashMap<String, String>>,
    assignments: Option<wardian_core::models::AutomationAssignments>,
    memory_principal: Option<String>,
}

fn automation_run_control_launch(
    request: ControlRequest,
) -> Result<AutomationRunControlLaunch, ControlError> {
    match request {
        ControlRequest::AutomationRun {
            path,
            provider,
            workspace,
            input,
            bindings,
            assignments,
            caller_agent_id,
            memory_capability,
        } => Ok(AutomationRunControlLaunch {
            path,
            provider,
            workspace,
            input,
            bindings,
            assignments,
            memory_principal: authenticate_automation_memory_principal(
                caller_agent_id.as_deref(),
                memory_capability.as_deref(),
            )?,
        }),
        _ => Err(ControlError::bad_request(
            "expected automation_run control request",
        )),
    }
}

fn authenticate_automation_memory_principal(
    caller_agent_id: Option<&str>,
    memory_capability: Option<&str>,
) -> Result<Option<String>, ControlError> {
    match (caller_agent_id, memory_capability) {
        (None, None) => Ok(None),
        (Some(agent_id), Some(capability)) => {
            let agent_id = agent_id.trim();
            let capability = capability.trim();
            if agent_id.is_empty() || capability.is_empty() {
                return Err(ControlError::bad_request(
                    "automation memory authority is incomplete",
                ));
            }
            let valid = wardian_core::memory::MemoryStore::from_default_home()
                .and_then(|store| store.validate_capability(agent_id, capability))
                .map_err(|_| {
                    ControlError::request_failed(
                        "automation memory authority could not be validated",
                    )
                })?;
            if !valid {
                return Err(ControlError::bad_request(
                    "automation memory authority is invalid",
                ));
            }
            Ok(Some(agent_id.to_string()))
        }
        _ => Err(ControlError::bad_request(
            "automation memory authority is incomplete",
        )),
    }
}

async fn handle_automation_run_control(
    app: &AppHandle,
    launch: AutomationRunControlLaunch,
) -> Result<String, ControlError> {
    let result = crate::commands::automation::automation_run_from_control(
        app.state::<AppState>(),
        app.clone(),
        launch.path,
        launch.provider,
        launch.workspace,
        launch.input,
        launch.bindings,
        launch.assignments,
        launch.memory_principal,
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
            provider_input_state: None,
            recovery: None,
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
    let stalled_composer =
        crate::delivery::codex_composer::session_has_stalled_composer(state.inner(), session_id)
            .await
            .unwrap_or(false);
    if stalled_composer {
        reasons.push("provider_composer_stalled".to_string());
    }
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
        restart_required: stalled_composer,
        reasons,
        provider_input_state: Some(if stalled_composer {
            "stalled_composer".to_string()
        } else {
            "no_stalled_composer_detected".to_string()
        }),
        recovery: stalled_composer.then(|| format!("wardian agent restart {session_id}")),
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

pub(crate) async fn deliver_prompt_to_agent(
    app: Option<&AppHandle>,
    state: &AppState,
    target: &str,
    prompt: &str,
    input_mode: MessageInputMode,
) -> Result<DeliveryDetail, ControlError> {
    let delivery = deliver_message_to_target_with_headless_timeout(
        app,
        state,
        target,
        prompt,
        None,
        input_mode,
        QueuePolicy::QueueIfBusy,
        None,
        None,
        false,
        crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
    )
    .await?;

    record_conversation_delivery(state, &delivery, prompt, None).await;
    delivery.into_iter().next().ok_or_else(|| {
        ControlError::request_failed(format!(
            "prompt delivery produced no result for target: {target}"
        ))
    })
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
    deliver_message_to_target_with_delivery_options(
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
        headless_timeout,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn deliver_message_to_target_with_delivery_options(
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
    orchestration: Option<&wardian_core::control::OrchestrationDeliveryOptions>,
    parent_interaction_id: Option<&str>,
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
    let mut failures = Vec::new();
    let mut delivery = Vec::with_capacity(session_ids.len());
    for initial_info in target_infos {
        let target_lifecycle_guard = match state.try_lock_agent_lifecycle(&initial_info.uuid).await
        {
            Some(guard) => guard,
            None => state.lock_agent_lifecycle(&initial_info.uuid).await,
        };
        let info = delivery_target_info(state, &initial_info.uuid).await?;
        if !same_delivery_target_incarnation(&initial_info, &info) {
            failures.push(format!("{}: target_replaced", initial_info.uuid));
            delivery.push(rejected_delivery_detail(
                initial_info,
                "target_replaced",
                input_mode,
                queue_policy,
            ));
            continue;
        }
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
        let live_surface_available = state
            .terminal_sessions
            .broker_state(&info.uuid)
            .await
            .is_ok();
        let route = if input_mode == MessageInputMode::ApprovalAction
            || matches!(queue_policy, QueuePolicy::MailboxOnly)
        {
            decide_delivery_route(&info.status, input_mode, queue_policy, approval_action)
        } else if should_route_native_without_live_surface(
            &info.provider,
            input_mode,
            queue_policy,
            live_surface_available,
        ) {
            DeliveryRoute::Headless
        } else if !status_uses_headless_delivery(&info.status)
            && provider_input_has_known_not_ready_state(state, &info.uuid).await
            && !provider_idle_status_allows_live_delivery(&info, queue_policy)
        {
            match queue_policy {
                QueuePolicy::QueueIfBusy => DeliveryRoute::Reject {
                    failure: "provider_input_not_ready",
                },
                QueuePolicy::LiveOnly => DeliveryRoute::Reject {
                    failure: "not_input_ready",
                },
                QueuePolicy::MailboxOnly => unreachable!("handled above"),
            }
        } else if active_conversation_lease_for_delivery(&info) {
            match queue_policy {
                QueuePolicy::QueueIfBusy => DeliveryRoute::Reject {
                    failure: "conversation_leased",
                },
                QueuePolicy::LiveOnly => DeliveryRoute::Reject {
                    failure: "conversation_leased",
                },
                QueuePolicy::MailboxOnly => unreachable!("handled above"),
            }
        } else {
            decide_delivery_route(&info.status, input_mode, queue_policy, approval_action)
        };
        let existing_native_interaction_id = if matches!(route, DeliveryRoute::Headless)
            && input_mode == MessageInputMode::Message
            && crate::delivery::native_session::NativeProviderProtocol::for_provider(&info.provider)
                .is_some()
        {
            orchestration
                .and_then(|options| options.idempotency_key.as_deref())
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(|key| {
                    wardian_core::db::native_delivery_by_idempotency(
                        sender_session_id.as_deref(),
                        &info.uuid,
                        orchestration
                            .map(|options| options.operation)
                            .unwrap_or_default(),
                        key,
                    )
                    .map_err(|error| ControlError::request_failed(error.to_string()))
                    .map(|record| record.map(|record| record.envelope.interaction_id))
                })
                .transpose()?
                .flatten()
        } else {
            None
        };
        let interaction_id = if let Some(interaction_id) = existing_native_interaction_id {
            interaction_id
        } else {
            state
                .interactions
                .create_message_durable(
                    sender_session_id,
                    vec![info.uuid.clone()],
                    InteractionBodyRef::Inline {
                        body: outbound_message.clone(),
                    },
                )
                .await
                .map_err(ControlError::request_failed)?
                .id
        };
        if let Some(app) = app {
            let _ = app.emit("pair-activity-changed", ());
        }
        match route {
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
                        require_provider_turn_receipt: true,
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
                if input_mode == MessageInputMode::Message
                    && info.provider != "codex"
                    && crate::delivery::native_session::NativeProviderProtocol::for_provider(
                        &info.provider,
                    )
                    .is_some()
                {
                    match deliver_native_message(
                        state,
                        &info,
                        &interaction_id,
                        &outbound_message,
                        input_mode,
                        queue_policy,
                        origin,
                        orchestration,
                        parent_interaction_id,
                    )
                    .await
                    {
                        Ok(detail) => {
                            delivered += 1;
                            record_delivery_attempt(state, &detail).await;
                            delivery.push(detail);
                            continue;
                        }
                        Err(failure) if failure.provider_boundary_crossed => {
                            failures.push(format!("{}: {}", info.uuid, failure.message));
                            let detail = native_delivery_failure_detail(
                                &info,
                                &interaction_id,
                                input_mode,
                                queue_policy,
                                &failure,
                            );
                            persist_interaction_delivery_attempt(
                                state,
                                &interaction_id,
                                &info.uuid,
                                DeliveryTransportKind::NativeProvider,
                                &detail,
                            )
                            .await;
                            record_delivery_attempt(state, &detail).await;
                            delivery.push(detail);
                            continue;
                        }
                        Err(_) => {
                            // Native negotiation/startup failed before the
                            // provider message boundary. The explicit reduced
                            // headless fallback remains safe here.
                        }
                    }
                }
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
                        lifecycle_guard: Some(target_lifecycle_guard),
                        orchestration,
                        parent_interaction_id,
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
                        failures.push(format!("{}: conversation_leased", current_info.uuid));
                        delivery.push(rejected_delivery_detail(
                            *current_info,
                            "conversation_leased",
                            input_mode,
                            queue_policy,
                        ));
                    }
                }
            }
        }
    }
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

#[allow(clippy::too_many_arguments)]
async fn deliver_native_message(
    state: &AppState,
    info: &DeliveryTargetInfo,
    interaction_id: &str,
    message: &str,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    origin: Option<&MessageOrigin>,
    orchestration: Option<&wardian_core::control::OrchestrationDeliveryOptions>,
    parent_interaction_id: Option<&str>,
) -> Result<DeliveryDetail, crate::delivery::native_broker::NativeBrokerError> {
    let latest_binding = wardian_core::db::latest_native_session_binding(&info.uuid)
        .map_err(|error| crate::delivery::native_broker::NativeBrokerError {
            code: wardian_core::native_transport::NativeDeliveryErrorCode::TransportUnavailable,
            message: format!("failed to read native session binding: {error}"),
            provider_boundary_crossed: false,
        })?
        .filter(|binding| binding.provider == info.provider);
    let current_generation = state
        .interactions
        .current_provider_input_generation(&info.uuid)
        .await;
    let generation = match latest_binding.as_ref() {
        Some(binding)
            if current_generation.is_none() || current_generation == Some(binding.generation) =>
        {
            binding.generation
        }
        _ => {
            state
                .interactions
                .start_provider_input_generation(&info.uuid, ProviderInputReadiness::Booting, None)
                .await
                .generation
        }
    };
    if let Some(expected) = orchestration.and_then(|options| options.expected_generation) {
        if expected != generation {
            return Err(crate::delivery::native_broker::NativeBrokerError {
                code: wardian_core::native_transport::NativeDeliveryErrorCode::StaleGeneration,
                message: format!(
                    "expected generation {expected}, but target {} is generation {generation}",
                    info.uuid
                ),
                provider_boundary_crossed: false,
            });
        }
    }
    let sender_agent_id =
        origin.map(|MessageOrigin::WardianAgent { session_id }| session_id.clone());
    let options = orchestration.cloned().unwrap_or_default();
    let record = state
        .native_delivery
        .admit(crate::delivery::native_broker::NativeDeliveryAdmission {
            interaction_id: interaction_id.to_string(),
            message_id: interaction_id.to_string(),
            target_agent_id: info.uuid.clone(),
            sender_agent_id,
            provider: info.provider.clone(),
            generation,
            operation: options.operation,
            caller_idempotency_key: options.idempotency_key,
            parent_interaction_id: parent_interaction_id.map(str::to_string),
            deadline_at: options.deadline_at,
            body: message.to_string(),
        })
        .await?;
    let receipt = state
        .native_delivery
        .dispatch(
            crate::delivery::native_broker::NativeSessionSpec {
                target_agent_id: info.uuid.clone(),
                provider: info.provider.clone(),
                generation,
                workspace: info.cwd.clone(),
                config: info.config.clone(),
            },
            record,
        )
        .await?;
    let accepted_only = info.provider == "codex"
        && receipt.record.phase
            == wardian_core::native_transport::NativeDeliveryPhase::ProviderAccepted;
    let detail = DeliveryDetail {
        uuid: info.uuid.clone(),
        name: info.name.clone(),
        provider: info.provider.clone(),
        runtime_state: "native_provider_session".to_string(),
        delivery_state: "provider_accepted".to_string(),
        input_mode,
        queue_policy,
        message_id: Some(receipt.record.envelope.interaction_id.clone()),
        delivery_phase: Some(
            if accepted_only {
                "provider_accepted"
            } else {
                "turn_started"
            }
            .to_string(),
        ),
        observed_state: Some(
            if accepted_only {
                "protocol_acknowledged"
            } else {
                "turn_started"
            }
            .to_string(),
        ),
        reason: Some(format!(
            "provider delivery evidence via {}",
            receipt.capabilities.transport
        )),
        profile: Some(receipt.capabilities.provider.clone()),
        error: None,
    };
    state
        .interactions
        .record_delivery_attempt_durable(
            &receipt.record.envelope.interaction_id,
            &info.uuid,
            DeliveryTransportKind::NativeProvider,
            generation,
            &detail.runtime_state,
            &detail.delivery_state,
            detail.delivery_phase.clone(),
            detail.observed_state.clone(),
            detail.reason.clone(),
            None,
        )
        .await
        .map_err(
            |message| crate::delivery::native_broker::NativeBrokerError {
                code: wardian_core::native_transport::NativeDeliveryErrorCode::TransportUnavailable,
                message,
                provider_boundary_crossed: true,
            },
        )?;
    let _ = state
        .interactions
        .update_message_status_durable(
            &receipt.record.envelope.interaction_id,
            InteractionStatus::Delivered,
        )
        .await;
    Ok(detail)
}

fn native_delivery_failure_detail(
    info: &DeliveryTargetInfo,
    interaction_id: &str,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    failure: &crate::delivery::native_broker::NativeBrokerError,
) -> DeliveryDetail {
    DeliveryDetail {
        uuid: info.uuid.clone(),
        name: info.name.clone(),
        provider: info.provider.clone(),
        runtime_state: "native_provider_session".to_string(),
        delivery_state: if failure.provider_boundary_crossed {
            "submitted_unconfirmed".to_string()
        } else {
            "failed".to_string()
        },
        input_mode,
        queue_policy,
        message_id: Some(interaction_id.to_string()),
        delivery_phase: Some(if failure.provider_boundary_crossed {
            "submitted_unconfirmed".to_string()
        } else {
            "failed_before_submit".to_string()
        }),
        observed_state: None,
        reason: Some(if failure.provider_boundary_crossed {
            "provider boundary may have been crossed; automatic retry and fallback are disabled"
                .to_string()
        } else {
            "native transport failed before provider submission".to_string()
        }),
        profile: Some(info.provider.clone()),
        error: Some(DeliveryErrorDetail {
            code: serde_json::to_value(&failure.code)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "native_delivery_failed".to_string()),
            message: failure.message.clone(),
        }),
    }
}

fn should_route_native_without_live_surface(
    provider: &str,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    live_surface_available: bool,
) -> bool {
    !live_surface_available
        && input_mode == MessageInputMode::Message
        && matches!(queue_policy, QueuePolicy::QueueIfBusy)
        && crate::delivery::native_session::NativeProviderProtocol::for_provider(provider).is_some()
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
        return DeliveryRoute::Reject {
            failure: "mailbox_only",
        };
    }

    match status {
        "idle" => DeliveryRoute::Live,
        "processing" => match queue_policy {
            QueuePolicy::QueueIfBusy => DeliveryRoute::Reject {
                failure: "target_processing",
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
                DeliveryRoute::Reject {
                    failure: "target_action_required",
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
            QueuePolicy::QueueIfBusy | QueuePolicy::MailboxOnly => DeliveryRoute::Reject {
                failure: "queued_not_live",
            },
            QueuePolicy::LiveOnly => DeliveryRoute::Reject {
                failure: "target_not_live",
            },
        },
        "headless" => match queue_policy {
            QueuePolicy::QueueIfBusy => DeliveryRoute::Reject {
                failure: "conversation_leased",
            },
            QueuePolicy::LiveOnly => DeliveryRoute::Reject {
                failure: "conversation_leased",
            },
            QueuePolicy::MailboxOnly => unreachable!("handled above"),
        },
        _ => DeliveryRoute::Reject {
            failure: "not_input_ready",
        },
    }
}

fn status_uses_headless_delivery(status: &str) -> bool {
    matches!(status, "off" | "error" | "headless")
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

enum HeadlessMessageDelivery {
    Completed(Box<DeliveryDetail>),
    Busy(Box<DeliveryTargetInfo>),
}

#[derive(Debug)]
enum HeadlessMessageLeaseError {
    Busy,
    Failed(String),
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
    // A provider status event can report idle while Codex still has an
    // unsubmitted bracketed paste in its composer. Inspect that provider-owned
    // surface before trusting the durable Ready observation; otherwise every
    // later delivery appends to the same poisoned composer.
    if info.provider == "codex"
        && crate::delivery::codex_composer::session_has_stalled_composer(state, &info.uuid).await?
    {
        return Err(format!(
            "Agent {} has an unsubmitted Codex payload in its composer; run `wardian agent restart {}` to clear it without deleting the agent or its history",
            info.uuid, info.uuid
        ));
    }
    if info.provider == "codex"
        && startup_readiness::codex_current_screen_requires_choice(state, &info.uuid).await?
    {
        return Err(format!(
            "Agent {} requires an explicit Codex model choice; no prompt bytes sent",
            info.uuid
        ));
    }
    // Cached Ready can come from a previous turn in OpenCode's rolling log.
    // The current composer must authorize input even when that cache is Ready.
    if info.provider == "opencode" {
        return wait_for_opencode_terminal_ready(state, &info.uuid, 15_000).await;
    }
    if provider_input_current_state(state, &info.uuid).await == Some(ProviderInputReadiness::Ready)
    {
        return Ok(());
    }

    if info.provider == "codex" {
        wait_for_terminal_output(state, &info.uuid, 15_000, |output| {
            provider_output_has_ready_prompt("codex", output)
        })
        .await
    } else if info.provider == "claude" {
        if current_agent_status_is_idle(state, &info.uuid).await? {
            Ok(())
        } else {
            wait_for_terminal_output(state, &info.uuid, 15_000, |output| {
                provider_output_has_ready_prompt("claude", output)
            })
            .await
        }
    } else if info.provider == "gemini" {
        wait_for_terminal_output(state, &info.uuid, 15_000, |output| {
            provider_output_has_ready_prompt("gemini", output)
        })
        .await
    } else if info.provider == "antigravity" {
        wait_for_terminal_output(state, &info.uuid, 15_000, |output| {
            provider_output_has_ready_prompt("antigravity", output)
        })
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

async fn provider_input_blocks_task_dispatch(state: &AppState, session_id: &str) -> bool {
    provider_input_current_state(state, session_id)
        .await
        .is_some_and(|input_state| input_state != ProviderInputReadiness::Ready)
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
    let generation = state
        .interactions
        .current_provider_input_generation(session_id)
        .await
        .unwrap_or(0);
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(timeout_ms) {
        let current_status = {
            let agents = state.agents.lock().await;
            let agent = agents
                .get(session_id)
                .ok_or_else(|| format!("Agent {} not found or is off", session_id))?;
            agent.current_status.clone()
        };
        let status = current_status
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        if wardian_core::identity::normalize_status(&status) == "idle"
            && startup_readiness::opencode_current_screen_is_ready(state, session_id).await?
            && record_provider_ready_evidence(
                state,
                session_id,
                generation,
                ProviderReadyEvidence::PromptDetected,
            )
            .await
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
    let generation = state
        .interactions
        .current_provider_input_generation(session_id)
        .await
        .unwrap_or(0);
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
        if is_ready(&output)
            && record_provider_ready_evidence(
                state,
                session_id,
                generation,
                ProviderReadyEvidence::PromptDetected,
            )
            .await
        {
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

pub(crate) fn provider_output_has_ready_prompt(provider: &str, output: &str) -> bool {
    match provider {
        "codex" => crate::delivery::codex_composer::output_has_ready_prompt(output),
        "claude" => claude_output_has_ready_prompt(output),
        "gemini" => gemini_output_has_ready_prompt(output),
        "antigravity" => antigravity_output_has_ready_prompt(output),
        _ => false,
    }
}

fn antigravity_ready_prompt_footer_line(line: &str) -> bool {
    line.contains("Press up to edit queued messages") || line.contains("? for shortcuts")
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
        cwd: crate::utils::fs::resolve_cwd(&config.folder, &config.session_id),
        config: config.clone(),
        config_identity: agent.config.clone(),
        // Provider log activity can outlive a leased background owner. Off is
        // authoritative for routing even when the observed status says Idle;
        // callers still enforce lease ownership before starting background work.
        status: if config.is_off {
            "off".to_string()
        } else {
            normalize_status(&status)
        },
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

/// Only canonical v2 work is eligible at status and restore opportunities.
pub(crate) fn spawn_agent_messaging_if_idle(app: &AppHandle, session_id: &str, status: &str) {
    if matches!(normalize_status(status).as_str(), "idle" | "off") {
        agent_messaging::spawn_pending_tasks(app, session_id);
    }
}

pub(crate) fn spawn_agent_messaging_after_restore(app: &AppHandle, session_id: &str) {
    agent_messaging::spawn_pending_tasks(app, session_id);
}

pub(crate) async fn dispatch_agent_messaging_from_status_observation(
    app: Option<&AppHandle>,
    state: &AppState,
    session_id: &str,
) {
    let _ = agent_messaging::push_native_information(state, session_id).await;
    let _ = agent_messaging::dispatch_pending_queue(app, state, session_id).await;
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

/// Shared body for the four `Topology*` requests; maps `commands::topology`'s
/// domain error to this module's wire error codes.
async fn topology_mutation(
    app: &AppHandle,
    op: wardian_core::topology::TopologyOperation,
    a: String,
    b: String,
    caller_session_id: Option<String>,
) -> Result<String, ControlError> {
    use crate::commands::topology::TopologyControlError;
    let response =
        crate::commands::topology::dispatch_topology_mutation(app, op, a, b, caller_session_id)
            .await
            .map_err(|error| match error {
                TopologyControlError::UnknownCaller => {
                    ControlError::not_found("caller session is not a known agent")
                }
                TopologyControlError::SelfServeRequired => ControlError::coded(
                    "self_serve_required",
                    "Inside a session, graph edits must involve the calling agent",
                ),
                TopologyControlError::Io(message) => ControlError::request_failed(message),
            })?;
    ok_json(&response)
}

/// Carries a browser failure's own code onto the wire.
///
/// `snapshot_stale` in particular has to survive the trip intact: it is the
/// signal that tells an agent to re-snapshot rather than retry.
fn browser_control_error(error: crate::state::browser_session::BrowserError) -> ControlError {
    ControlError::coded(error.code(), error.to_string())
}

fn native_broker_control_error(
    error: crate::delivery::native_broker::NativeBrokerError,
) -> ControlError {
    let code = match error.code {
        wardian_core::native_transport::NativeDeliveryErrorCode::UnsupportedProvider => {
            "native_transport_unsupported"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::IdempotencyConflict => {
            "idempotency_conflict"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::StaleGeneration => {
            "stale_generation"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::DeadlineExpired => {
            "deadline_expired"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::UnsupportedOperation => {
            "native_operation_unsupported"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::CapabilityUnavailable => {
            "native_capability_unavailable"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::TransportUnavailable => {
            "native_session_unavailable"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::FailedBeforeSubmit => {
            "native_failed_before_submit"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::SubmittedUnconfirmed => {
            "native_submitted_unconfirmed"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::InvalidTransition => {
            "invalid_delivery_transition"
        }
        wardian_core::native_transport::NativeDeliveryErrorCode::NotFound => "not_found",
    };
    ControlError::coded(code, error.message).with_details(serde_json::json!({
        "provider_boundary_crossed": error.provider_boundary_crossed,
    }))
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
pub(crate) struct ControlError {
    code: &'static str,
    message: String,
    details: Option<serde_json::Value>,
}

impl ControlError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
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

    pub(crate) fn request_failed(message: impl ToString) -> Self {
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
    // Lease loading performs synchronous filesystem I/O. Keep it completely
    // outside the live-agent locks so a slow state file cannot stall delivery
    // or other agent lifecycle operations.
    let active_leases = wardian_core::conversation_lease::load_leases();
    let lease_now = chrono::Utc::now().to_rfc3339();
    let order = state.agent_order.lock().await.clone();
    let agents = state.agents.lock().await;
    // Only copy the small, independently-owned snapshot inputs while the map
    // lock is held. Per-agent mutexes and snapshot construction happen after
    // releasing the global map lock.
    let agent_sources = collect_agent_snapshot_sources(&agents);
    drop(agents);

    let mut snapshots = Vec::with_capacity(agent_sources.len());
    let mut seen = std::collections::HashSet::new();

    for session_id in order {
        if let Some(source) = agent_sources.get(&session_id) {
            snapshots.push(snapshot_agent_source(source, &active_leases, &lease_now));
            seen.insert(session_id);
        }
    }

    for (session_id, source) in &agent_sources {
        if !seen.contains(session_id) {
            snapshots.push(snapshot_agent_source(source, &active_leases, &lease_now));
        }
    }

    wardian_core::identity::append_missing_persisted_agents(snapshots, &seen)
}

fn collect_agent_snapshot_sources(
    agents: &std::collections::HashMap<String, crate::state::ActiveAgent>,
) -> std::collections::HashMap<String, AgentSnapshotSource> {
    agents
        .iter()
        .map(|(session_id, agent)| (session_id.clone(), AgentSnapshotSource::from(agent)))
        .collect()
}

fn snapshot_agent(agent: &crate::state::ActiveAgent) -> AgentIdentity {
    let active_leases = wardian_core::conversation_lease::load_leases();
    let lease_now = chrono::Utc::now().to_rfc3339();
    snapshot_agent_with_leases(agent, &active_leases, &lease_now)
}

#[derive(Clone)]
struct AgentSnapshotSource {
    config: Arc<Mutex<wardian_core::models::AgentConfig>>,
    current_status: Arc<Mutex<String>>,
    init_timestamp: Arc<Mutex<Option<String>>>,
    last_status_at: Arc<Mutex<Option<String>>>,
    process_id: Option<u32>,
}

impl From<&crate::state::ActiveAgent> for AgentSnapshotSource {
    fn from(agent: &crate::state::ActiveAgent) -> Self {
        Self {
            config: agent.config.clone(),
            current_status: agent.current_status.clone(),
            init_timestamp: agent.init_timestamp.clone(),
            last_status_at: agent.last_status_at.clone(),
            process_id: agent.process_id,
        }
    }
}

fn snapshot_agent_with_leases(
    agent: &crate::state::ActiveAgent,
    active_leases: &[wardian_core::conversation_lease::ConversationLease],
    lease_now: &str,
) -> AgentIdentity {
    snapshot_agent_source(&AgentSnapshotSource::from(agent), active_leases, lease_now)
}

fn snapshot_agent_source(
    source: &AgentSnapshotSource,
    active_leases: &[wardian_core::conversation_lease::ConversationLease],
    lease_now: &str,
) -> AgentIdentity {
    let config = source
        .config
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let status = source
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
    let started_at = source
        .init_timestamp
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let last_status_at = source
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
        pid: source.process_id,
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
pub(crate) mod test_support;

#[cfg(test)]
pub(crate) mod tests {
    include!("control/tests/registrations.rs");
    include!("control/tests/lifecycle_delivery.rs");

    use super::*;
    use crate::state::ActiveAgent;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::{Arc, Mutex};
    use wardian_core::models::{
        AgentConfig, AgentConversationMode, AutomationRoleAssignment, BusyPolicy,
    };

    use super::test_support::TestWardianHome;

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
        let _home = TestWardianHome::new_async().await;

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
    fn automation_run_control_launch_forwards_launch_options() {
        let mut assignments = wardian_core::models::AutomationAssignments::new();
        assignments.insert(
            "reviewer".to_string(),
            AutomationRoleAssignment::Agent {
                agent_id: "agent-1".to_string(),
                conversation: AgentConversationMode::Current,
                busy_policy: BusyPolicy::Fail,
            },
        );
        let bindings = HashMap::from([("legacy".to_string(), "mock".to_string())]);
        let input = serde_json::json!({"target":"HEAD"});
        let request = ControlRequest::AutomationRun {
            path: "/automation/controlwf.md".to_string(),
            provider: Some("mock".to_string()),
            workspace: Some("/workspace".to_string()),
            input: Some(input.clone()),
            bindings: Some(bindings.clone()),
            assignments: Some(assignments.clone()),
            caller_agent_id: None,
            memory_capability: None,
        };

        let launch = automation_run_control_launch(request).unwrap();

        assert_eq!(launch.path, "/automation/controlwf.md");
        assert_eq!(launch.provider.as_deref(), Some("mock"));
        assert_eq!(launch.workspace.as_deref(), Some("/workspace"));
        assert_eq!(launch.input, Some(input));
        assert_eq!(launch.bindings, Some(bindings));
        assert_eq!(launch.assignments, Some(assignments));
        assert_eq!(launch.memory_principal, None);
    }

    #[test]
    fn automation_memory_principal_requires_agent_bound_capability() {
        let _home = TestWardianHome::new();
        let store = wardian_core::memory::MemoryStore::from_default_home().unwrap();
        let capability = store.issue_process_capability("agent-a").unwrap();

        assert_eq!(
            authenticate_automation_memory_principal(Some("agent-a"), Some(capability.token()),)
                .unwrap(),
            Some("agent-a".to_string())
        );
        assert!(authenticate_automation_memory_principal(
            Some("agent-b"),
            Some(capability.token()),
        )
        .is_err());
        assert!(authenticate_automation_memory_principal(Some("agent-a"), None).is_err());
    }

    pub(crate) fn test_agent(
        session_id: &str,
        session_name: &str,
        agent_class: &str,
    ) -> ActiveAgent {
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
            memory_capability: None,
            runtime_generation: None,
            process_id: Some(1234),
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(Some("2026-05-07T00:00:00.000Z".to_string()))),
            last_query_timestamp: Arc::new(Mutex::new(None)),
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

    pub(super) async fn insert_test_agent(
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

    #[tokio::test]
    async fn snapshot_sources_release_global_agent_lock_before_per_agent_reads() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;

        let sources = {
            let agents = state.agents.lock().await;
            collect_agent_snapshot_sources(&agents)
        };
        let source = sources.get("agent-1").expect("snapshot source");
        let _config_guard = source.config.lock().expect("config lock");

        let _ = state
            .agents
            .try_lock()
            .expect("per-agent snapshot reads must not retain the global agent lock");
    }

    pub(super) async fn install_test_terminal_runtime(
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

    #[tokio::test]
    async fn human_prompt_busy_rejects_without_pty_or_legacy_admission() {
        let _home = TestWardianHome::new_async().await;
        let state = AppState::new();
        insert_test_agent(&state, "receiver", "Receiver", "Test").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        install_test_terminal_runtime(&state, "receiver", tx).await;
        assert!(deliver_prompt_to_agent(
            None,
            &state,
            "receiver",
            "human prompt",
            MessageInputMode::Message
        )
        .await
        .is_err());
        assert!(rx.try_recv().is_err());
        assert!(wardian_core::db::list_mailbox_messages()
            .unwrap()
            .is_empty());
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
    async fn codex_control_send_rejects_stalled_composer_even_when_state_is_ready() {
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
                .push_output(b"\r\n\xe2\x80\xba [Pasted Content 6479 chars]\r\n");
        }
        record_provider_ready_evidence(&state, "agent-1", 0, ProviderReadyEvidence::ProviderEvent)
            .await;
        let info = delivery_target_infos(&state, &["agent-1".to_string()])
            .await
            .unwrap()
            .remove(0);

        let error = wait_for_terminal_ready_for_control_send(&state, &info)
            .await
            .expect_err("stalled composer must override idle readiness");

        assert!(error.contains("unsubmitted Codex payload"));
        assert!(error.contains("wardian agent restart agent-1"));
    }

    include!("control/opencode_receipt_tests.rs");

    #[tokio::test]
    async fn message_delivery_archives_unconfirmed_live_input_with_agent_origin() {
        let _home = TestWardianHome::new_async().await;
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
        let _home = TestWardianHome::new_async().await;
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
        let _home = TestWardianHome::new_async().await;
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
        let _home = TestWardianHome::new_async().await;
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
    async fn fresh_headless_message_lease_marks_the_off_agent_headless() {
        let _home = TestWardianHome::new_async().await;
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
    async fn headless_status_observations_follow_the_lease_lifecycle() {
        let _home = TestWardianHome::new_async().await;
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
        let _home = TestWardianHome::new_async().await;
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
    async fn late_headless_completion_skips_a_replaced_agent_incarnation() {
        let _home = TestWardianHome::new_async().await;
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
        let _home = TestWardianHome::new_async().await;
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
    async fn ordinary_prompt_entry_starts_headless_execution_for_offline_agent() {
        if !node_available() {
            return;
        }
        let _home = TestWardianHome::new_async().await;
        let _scenario = ScopedEnvVar::set("WARDIAN_MOCK_SCENARIO", "headless");
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

        let detail = deliver_prompt_to_agent(
            None,
            &state,
            "CoderOne",
            "run offline",
            MessageInputMode::Message,
        )
        .await
        .expect("offline prompt delivery");

        assert_eq!(detail.runtime_state, "headless_process");
        assert_eq!(detail.delivery_state, "provider_applied");
        assert_eq!(detail.queue_policy, QueuePolicy::QueueIfBusy);
    }

    #[tokio::test]
    async fn configured_off_routing_ignores_status_drift_without_changing_input_policy() {
        let state = AppState::new();
        let agent = test_agent("off-routing", "Routing", "Test");
        {
            let mut config = agent.config.lock().unwrap();
            config.provider = "codex".into();
            config.is_off = true;
        }
        let config = agent.config.clone();
        let observed_status = agent.current_status.clone();
        state
            .agents
            .lock()
            .await
            .insert("off-routing".into(), agent);
        let approval = ApprovalAction::Accept;
        for drift in ["Processing...", "Idle", "Action Needed"] {
            *observed_status.lock().unwrap() = drift.into();
            let info = delivery_target_info(&state, "off-routing").await.unwrap();
            assert_eq!(info.status, "off", "observed status: {drift}");
            assert!(status_uses_headless_delivery(&info.status));
            for (mode, policy, action, expected) in [
                (
                    MessageInputMode::Message,
                    QueuePolicy::QueueIfBusy,
                    None,
                    DeliveryRoute::Headless,
                ),
                (
                    MessageInputMode::Message,
                    QueuePolicy::LiveOnly,
                    None,
                    DeliveryRoute::Reject {
                        failure: "target_not_live",
                    },
                ),
                (
                    MessageInputMode::Message,
                    QueuePolicy::MailboxOnly,
                    None,
                    DeliveryRoute::Reject {
                        failure: "mailbox_only",
                    },
                ),
                (
                    MessageInputMode::Command,
                    QueuePolicy::QueueIfBusy,
                    None,
                    DeliveryRoute::Reject {
                        failure: "queued_not_live",
                    },
                ),
                (
                    MessageInputMode::ApprovalAction,
                    QueuePolicy::QueueIfBusy,
                    Some(&approval),
                    DeliveryRoute::Reject {
                        failure: "not_input_ready",
                    },
                ),
            ] {
                assert_eq!(
                    decide_delivery_route(&info.status, mode, policy, action),
                    expected,
                    "observed status: {drift}, input: {mode:?}, policy: {policy:?}",
                );
            }
            // Routing does not rewrite telemetry or the configured lifecycle.
            assert_eq!(*observed_status.lock().unwrap(), drift);
            assert!(config.lock().unwrap().is_off);
        }
        config.lock().unwrap().is_off = false;
        for (observed, mode, action) in [
            ("Idle", MessageInputMode::Message, None),
            (
                "Action Needed",
                MessageInputMode::ApprovalAction,
                Some(&approval),
            ),
        ] {
            *observed_status.lock().unwrap() = observed.into();
            let info = delivery_target_info(&state, "off-routing").await.unwrap();
            assert!(!status_uses_headless_delivery(&info.status));
            assert_eq!(
                decide_delivery_route(&info.status, mode, QueuePolicy::LiveOnly, action),
                DeliveryRoute::Live,
            );
        }
    }

    #[test]
    fn human_prompt_rejects_processing_without_queueing() {
        let route = decide_delivery_route(
            "processing",
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Reject {
                failure: "target_processing"
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
    fn restored_native_agent_without_live_surface_reuses_native_transport() {
        assert!(should_route_native_without_live_surface(
            "claude",
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            false,
        ));
        assert!(!should_route_native_without_live_surface(
            "claude",
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            true,
        ));
        assert!(!should_route_native_without_live_surface(
            "gemini",
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            false,
        ));
    }

    #[test]
    fn human_prompt_rejects_an_active_headless_turn() {
        let route = decide_delivery_route(
            "headless",
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Reject {
                failure: "conversation_leased"
            }
        );
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
    fn human_prompt_rejects_off_provider_command() {
        let route = decide_delivery_route(
            "off",
            MessageInputMode::Command,
            QueuePolicy::QueueIfBusy,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Reject {
                failure: "queued_not_live"
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
    fn human_prompt_rejects_action_required_without_queueing() {
        let route = decide_delivery_route(
            "action_required",
            MessageInputMode::Message,
            QueuePolicy::QueueIfBusy,
            None,
        );

        assert_eq!(
            route,
            DeliveryRoute::Reject {
                failure: "target_action_required"
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
    async fn renamed_agent_is_immediately_resolvable_by_send_and_ask() {
        let state = AppState::new();
        insert_test_agent(&state, "agent-1", "CoderOne", "Coder").await;
        state
            .agents
            .lock()
            .await
            .get_mut("agent-1")
            .unwrap()
            .config
            .lock()
            .unwrap()
            .session_name = "RenamedCoder".to_string();

        assert_eq!(
            resolve_send_targets_scoped(&state, "RenamedCoder", None, false).await,
            vec!["agent-1".to_string()]
        );
        assert_eq!(
            resolve_target_uuid_in_state(&state, "RenamedCoder").await,
            Some("agent-1".to_string())
        );
        assert_eq!(resolve_target_uuid_in_state(&state, "CoderOne").await, None);
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
        let home = TestWardianHome::new_async().await;
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

        let result = wait_for_terminal_output(
            &state,
            "agent-1",
            1,
            crate::delivery::codex_composer::output_has_ready_prompt,
        )
        .await;

        assert!(result.is_err());
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
    async fn approval_send_failure_is_not_retry_safe_after_the_terminal_boundary() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);

        let error = submit_approval_action_via_sender(&tx, "codex", &ApprovalAction::Accept)
            .await
            .expect_err("closed input channel");

        assert_eq!(error.phase, "approval_send_failed");
        assert!(!error.retry_safe);
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
                owner_kind: "automation_run".to_string(),
                owner_id: "automation/fresh".to_string(),
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
