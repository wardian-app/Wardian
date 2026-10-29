//! Canonical messaging admission and automatic dispatch. Provider integrations
//! acquire the durable delivery claim before exposing canonical interaction text.
use super::*;
use crate::delivery::native_broker::{
    log_opencode_dispatch_diagnostic, OpenCodeDispatchDiagnosticReason,
    OpenCodeDispatchDiagnosticStage,
};
use wardian_core::agent_messaging::{
    AgentMessagingError, AgentMessagingRequest as Request, AgentMessagingResponse as Response,
};
use wardian_core::db::agent_messaging as store;
mod controller;
mod native;
pub(super) use controller::handle as handle_controller;

/// Restore both canonical work and original-conversation continuations.
pub(super) fn spawn_after_restore(app: &AppHandle, session_id: &str) {
    spawn_pending_tasks_when_ready(app, session_id, None);
    spawn_pending_reply_continuations(app, session_id);
}

/// Keep status-driven recovery ordering beside canonical delivery policy.
pub(super) async fn dispatch_from_status_observation(
    app: Option<&AppHandle>,
    state: &AppState,
    session_id: &str,
) {
    let _ = dispatch_pending_wakeable_information(app, state, session_id).await;
    let _ = push_native_information(state, session_id).await;
    let _ = dispatch_pending_reply_continuations(app, state, session_id, None).await;
    let _ = dispatch_pending_queue(app, state, session_id).await;
}

pub(super) async fn push_native_information(
    state: &AppState,
    recipient: &str,
) -> Result<(), ControlError> {
    native::push_pending_information(state, recipient).await
}

pub(super) async fn handle(
    app: &AppHandle,
    request: Request,
    origin: MessageOrigin,
) -> Result<String, ControlError> {
    let response = handle_in_state(Some(app), &app.state::<AppState>(), request, origin).await?;
    ok_json(&response)
}

async fn handle_in_state(
    app: Option<&AppHandle>,
    state: &AppState,
    request: Request,
    origin: MessageOrigin,
) -> Result<Response, ControlError> {
    let MessageOrigin::WardianAgent { session_id: sender } = origin;
    authenticate(state, &sender).await?;
    match request {
        Request::ListAgents => {
            let sources = {
                let agents = state.agents.lock().await;
                collect_agent_snapshot_sources(&agents)
            };
            let leases = wardian_core::conversation_lease::load_leases();
            let now = chrono::Utc::now().to_rfc3339();
            let agents = sources
                .values()
                .map(|source| snapshot_agent_source(source, &leases, &now))
                .collect();
            let topology = crate::utils::fs::get_wardian_home()
                .map(|home| wardian_core::topology::load_topology(&home))
                .unwrap_or_default();
            Ok(Response::ListAgents {
                agents: neighbor_agents(agents, &sender, &topology),
            })
        }
        Request::InterruptAgent { target } => native::interrupt(state, &target).await,
        Request::SendMessage {
            target,
            message,
            idempotency_key,
        } => {
            admit(
                app,
                state,
                AdmissionInput {
                    sender: &sender,
                    target: &target,
                    message: &message,
                    key: idempotency_key.as_deref(),
                    task: false,
                },
            )
            .await
        }
        Request::FollowupTask {
            target,
            message,
            idempotency_key,
        } => {
            admit(
                app,
                state,
                AdmissionInput {
                    sender: &sender,
                    target: &target,
                    message: &message,
                    key: idempotency_key.as_deref(),
                    task: true,
                },
            )
            .await
        }
        Request::Reply {
            request_id,
            status,
            message,
        } => {
            let replied = state
                .interactions
                .reply_agent_message(&sender, &request_id, status, &message)
                .await
                .map_err(control_error)?;
            if let Some(app) = app {
                let _ = app.emit("pair-activity-changed", ());
                if let Some(recipient) = replied.record.target_session_ids.first() {
                    spawn_reply_continuation(app, recipient, &replied.record.id);
                }
            }
            Ok(Response::Reply {
                request_id,
                interaction_id: replied.record.id,
                delivery_state: "stored".into(),
                duplicate: replied.duplicate,
            })
        }
        Request::ReceiveMessages { .. } | Request::WaitAgent { .. } => Err(ControlError::coded(
            "automatic_delivery",
            "Agent-facing mailbox receive and wait are retired; Wardian delivers actionable replies automatically.",
        )),
    }
}

fn neighbor_agents(
    agents: Vec<AgentIdentity>,
    sender: &str,
    topology: &wardian_core::topology::Topology,
) -> Vec<AgentIdentity> {
    let refs = agents
        .iter()
        .map(|agent| wardian_core::topology::AgentRef {
            uuid: agent.uuid.clone(),
            workspace: agent.workspace.clone(),
        })
        .collect::<Vec<_>>();
    let neighbors = wardian_core::topology::resolve_neighbors(sender, topology, &refs);
    let reasons = neighbors
        .members
        .into_iter()
        .map(|member| (member.uuid, member.reasons.join(",")))
        .collect::<std::collections::HashMap<_, _>>();
    let mut agents = agents
        .into_iter()
        .filter(|agent| agent.uuid == sender || reasons.contains_key(&agent.uuid))
        .map(|mut agent| {
            agent.visibility = reasons.get(&agent.uuid).cloned();
            agent
        })
        .collect::<Vec<_>>();
    agents.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    agents
}

async fn authenticate(state: &AppState, sender: &str) -> Result<(), ControlError> {
    if sender.is_empty() || !state.agents.lock().await.contains_key(sender) {
        return Err(ControlError::coded(
            "unauthorized",
            "A current managed agent origin is required.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerProviderClassification {
    OpenCode,
    Other,
    Unknown,
}

impl WorkerProviderClassification {
    fn from_config(provider: &str) -> Self {
        let provider = provider.trim();
        if provider.is_empty() {
            Self::Unknown
        } else if provider.eq_ignore_ascii_case("opencode") {
            Self::OpenCode
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedRecipient {
    id: String,
    provider: WorkerProviderClassification,
}

async fn resolve_exact(state: &AppState, target: &str) -> Result<ResolvedRecipient, ControlError> {
    let selector = target.to_ascii_lowercase();
    if target.trim().is_empty()
        || target != target.trim()
        || matches!(selector.as_str(), "all" | "*" | "broadcast")
        || selector.starts_with("class:")
    {
        return Err(ControlError::coded(
            "invalid_target",
            "Use one exact agent name or UUID.",
        ));
    }
    let sources = {
        let agents = state.agents.lock().await;
        collect_agent_snapshot_sources(&agents)
    };
    let mut matches = Vec::new();
    for (id, agent) in sources.iter() {
        let config = agent
            .config
            .lock()
            .map_err(|_| ControlError::request_failed("Agent configuration lock poisoned."))?;
        if id == target || config.session_name == target {
            matches.push((
                id.clone(),
                WorkerProviderClassification::from_config(&config.provider),
            ));
        }
    }
    match matches.as_slice() {
        [(id, provider)] => Ok(ResolvedRecipient {
            id: id.clone(),
            provider: *provider,
        }),
        [] => Err(ControlError::not_found("No exact recipient exists.")),
        _ => Err(ControlError::coded(
            "ambiguous_target",
            "More than one agent matches; use an unambiguous UUID.",
        )),
    }
}

/// Automation uses the same exact name/UUID resolver as managed messaging.
/// Rechecking before admission plus the interaction deletion guard closes the
/// gap between launch preflight and a later delivery.
pub(crate) async fn resolve_automation_recipient(
    state: &AppState,
    target: &str,
) -> Result<String, ControlError> {
    let id = resolve_exact(state, target).await?.id;
    state
        .interactions
        .check_automation_recipient(&id)
        .await
        .map_err(control_error)?;
    Ok(id)
}

pub(crate) fn notify_information_admitted(
    app: &AppHandle,
    recipient: &str,
    admitted: &store::Admitted,
) {
    let _ = app.emit("pair-activity-changed", ());
    if !admitted.duplicate {
        native::spawn_information(app, recipient);
    }
}

pub(super) fn spawn_reply_continuation(app: &AppHandle, recipient: &str, reply_id: &str) {
    let app = app.clone();
    let recipient = recipient.to_owned();
    let reply_id = reply_id.to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let Err(error) =
            dispatch_pending_reply_continuations(Some(&app), &state, &recipient, Some(&reply_id))
                .await
        {
            manager::log_debug(&format!("[WARDIAN] reply continuation: {error}"));
        }
    });
}

pub(super) fn spawn_pending_reply_continuations(app: &AppHandle, recipient: &str) {
    let app = app.clone();
    let recipient = recipient.to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let Err(error) =
            dispatch_pending_reply_continuations(Some(&app), &state, &recipient, None).await
        {
            manager::log_debug(&format!("[WARDIAN] reply continuation recovery: {error}"));
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurableOriginMatch {
    Current,
    WaitingForIdentity,
    Stale,
}

fn durable_origin_matches(
    origin: &store::RequesterOrigin,
    current_provider: &str,
    current_conversation_id: Option<&str>,
) -> DurableOriginMatch {
    durable_conversation_matches(
        &origin.provider,
        &origin.conversation_id,
        current_provider,
        current_conversation_id,
    )
}

fn durable_conversation_matches(
    origin_provider: &str,
    origin_conversation_id: &str,
    current_provider: &str,
    current_conversation_id: Option<&str>,
) -> DurableOriginMatch {
    if !origin_provider.eq_ignore_ascii_case(current_provider) {
        return DurableOriginMatch::Stale;
    }
    match current_conversation_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(current) if current == origin_conversation_id => DurableOriginMatch::Current,
        Some(_) => DurableOriginMatch::Stale,
        None => DurableOriginMatch::WaitingForIdentity,
    }
}

fn surface_has_dedicated_turn_receipt(provider: &str) -> bool {
    matches!(provider, "pi" | "opencode")
}

/// Recover only messages explicitly marked wakeable when they were admitted.
/// Historical informational rows stay context-only, and a replaced provider
/// conversation permanently fences the original message from the new session.
pub(super) async fn dispatch_pending_wakeable_information(
    app: Option<&AppHandle>,
    state: &AppState,
    recipient: &str,
) -> Result<(), ControlError> {
    let ids = store::with_db(|conn| store::pending_wakeable_information(conn, recipient))
        .map_err(control_error)?;
    for id in ids {
        let origin = store::with_db(|conn| store::recipient_origin(conn, &id));
        let origin = match origin {
            Ok(Some(origin)) => origin,
            Ok(None) | Err(_) => {
                state
                    .interactions
                    .stale_agent_wakeable_information(&id)
                    .await
                    .map_err(control_error)?;
                continue;
            }
        };
        let Some(lifecycle) = state.try_lock_agent_lifecycle(recipient).await else {
            continue;
        };
        let info = match delivery_target_info(state, recipient).await {
            Ok(info) => info,
            Err(_) => continue,
        };
        match durable_conversation_matches(
            &origin.provider,
            &origin.conversation_id,
            &info.provider,
            info.resume_session.as_deref(),
        ) {
            DurableOriginMatch::Current => {}
            DurableOriginMatch::WaitingForIdentity => continue,
            DurableOriginMatch::Stale => {
                state
                    .interactions
                    .stale_agent_wakeable_information(&id)
                    .await
                    .map_err(control_error)?;
                continue;
            }
        }
        if info.config.is_off || info.status == "off" {
            continue;
        }
        if active_conversation_lease_for_delivery(&info) {
            continue;
        }

        let generation = state
            .interactions
            .current_provider_input_generation(recipient)
            .await
            .filter(|generation| *generation > 0);
        let Some(generation) = generation else {
            continue;
        };
        let idle = wardian_core::identity::normalize_status(&info.status) == "idle";
        let active_codex = info.provider == "codex"
            && wardian_core::identity::normalize_status(&info.status) == "processing";
        if !idle && !active_codex {
            continue;
        }

        let mut native_route = false;
        if info.provider == "codex" {
            if native::codex_input_is_ready(state, recipient, generation).await
                && state
                    .native_delivery
                    .codex_owner_selected(recipient, generation)
                    .await
            {
                match state
                    .native_delivery
                    .codex_binding(recipient, generation)
                    .await
                {
                    Ok(binding) if binding.provider_session_id == origin.conversation_id => {
                        native_route = true;
                    }
                    Ok(_) => {
                        state
                            .interactions
                            .stale_agent_wakeable_information(&id)
                            .await
                            .map_err(control_error)?;
                        continue;
                    }
                    Err(_) => continue,
                }
            }
        } else if idle {
            match info.provider.as_str() {
                "pi" if state
                    .native_delivery
                    .pi_bridge_prepared(recipient, generation)
                    .await =>
                {
                    match state
                        .native_delivery
                        .pi_binding(recipient, generation)
                        .await
                    {
                        Ok(binding)
                            if binding.provider_session_id.as_deref()
                                == Some(origin.conversation_id.as_str()) =>
                        {
                            native_route = true;
                        }
                        Ok(_) => {
                            state
                                .interactions
                                .stale_agent_wakeable_information(&id)
                                .await
                                .map_err(control_error)?;
                            continue;
                        }
                        Err(_) => continue,
                    }
                }
                "opencode" => {
                    match state
                        .native_delivery
                        .opencode_http_eligibility(recipient, generation)
                        .await
                    {
                        crate::delivery::native_broker::OpenCodeHttpEligibility::Ready => {
                            if state
                                .native_delivery
                                .opencode_http_admission(
                                    recipient,
                                    generation,
                                    &info.config,
                                    &info.cwd,
                                )
                                .await
                                .is_ok()
                            {
                                native_route = true;
                            } else {
                                continue;
                            }
                        }
                        crate::delivery::native_broker::OpenCodeHttpEligibility::Pending
                        | crate::delivery::native_broker::OpenCodeHttpEligibility::Failed => {
                            continue;
                        }
                        crate::delivery::native_broker::OpenCodeHttpEligibility::Unsupported => {}
                    }
                }
                _ => {}
            }
        }

        if active_codex && !native_route {
            continue;
        }

        let surface_runtime_generation = if !active_codex && !native_route {
            if provider_input_blocks_task_dispatch(state, recipient).await || app.is_none() {
                continue;
            }
            let Ok(broker_state) = state.terminal_sessions.broker_state(recipient).await else {
                continue;
            };
            if !surface_has_dedicated_turn_receipt(&info.provider)
                && !state
                    .terminal_sessions
                    .native_write_receipts_enabled(recipient)
                    .await
                    .unwrap_or(false)
            {
                continue;
            }
            Some(broker_state.runtime_generation)
        } else {
            None
        };

        let Some(claim) = state
            .interactions
            .claim_agent_wakeable_information(recipient, &id, generation)
            .await
            .map_err(control_error)?
        else {
            continue;
        };
        let context = prepare_claim_context(state, &claim).await?;
        if active_codex {
            drop(lifecycle);
            let result = state
                .native_delivery
                .codex_push(recipient, generation, &id, &context)
                .await;
            let settlement = result
                .as_ref()
                .map(|receipt| receipt.delivery_state.as_str())
                .map_err(|error| error.provider_boundary_crossed);
            native::settle(state, &claim, settlement).await?;
            if let Err(error) = result {
                manager::log_debug(&format!(
                    "[WARDIAN] informational context {}: {}",
                    id, error.message
                ));
            }
        } else if native_route {
            drop(lifecycle);
            let result =
                native::dispatch_attached_information(state, &info, &claim, &context).await;
            let settlement = match &result {
                Ok(state) if state == "failed_before_submit" => Err(false),
                Ok(state) => Ok(state.as_str()),
                Err(error) => Err(error.provider_boundary_crossed),
            };
            native::settle(state, &claim, settlement).await?;
            if let Err(error) = result {
                manager::log_debug(&format!(
                    "[WARDIAN] informational wake {}: {}",
                    id, error.message
                ));
            }
        } else {
            drop(lifecycle);
            let frame: wardian_core::agent_messaging::AgentMessageContext =
                serde_json::from_str(&context).map_err(|error| {
                    ControlError::request_failed(format!(
                        "informational message context was invalid: {error}"
                    ))
                })?;
            let prompt = wardian_core::agent_messaging::information_delivery_prompt(&frame);
            let result = crate::delivery::submit_live_surface_prompt(
                app,
                state,
                crate::delivery::LiveSurfacePromptRequest {
                    session_id: recipient.to_owned(),
                    prompt,
                    interaction_id: Some(claim.record.id.clone()),
                    input_mode: MessageInputMode::Message,
                    queue_policy: QueuePolicy::LiveOnly,
                    approval_action: None,
                    origin: claim
                        .record
                        .sender_session_id
                        .clone()
                        .map(|session_id| MessageOrigin::WardianAgent { session_id }),
                    write_fence: Some(
                        crate::utils::delivery_transaction::AutomatedInputWriteFence {
                            provider: info.provider.clone(),
                            provider_conversation_id: origin.conversation_id.clone(),
                            provider_input_generation: generation,
                            terminal_runtime_generation: surface_runtime_generation
                                .expect("surface route captured its terminal generation"),
                            agent_config_identity: info.config_identity.clone(),
                        },
                    ),
                    runtime_state: "agent_messaging_information_wake",
                    mark_prompt_started: true,
                    require_provider_turn_receipt: true,
                    payload_sent_detail: None,
                    delivery_message_id: Some(claim.record.id.clone()),
                },
            )
            .await;
            let settlement = match &result {
                Ok(_) => Ok("provider_visible"),
                Err(error) if error.retry_safe => Err(false),
                Err(_) => Err(true),
            };
            native::settle(state, &claim, settlement).await?;
            if let Err(error) = result {
                manager::log_debug(&format!(
                    "[WARDIAN] informational wake {}: {}",
                    id, error.message
                ));
            }
        }
    }
    Ok(())
}

/// Recover canonical replies against their durable requester conversation.
/// The admission generation is evidence only: normal provider restoration
/// advances the runtime generation while preserving the conversation identity.
pub(super) async fn dispatch_pending_reply_continuations(
    app: Option<&AppHandle>,
    state: &AppState,
    recipient: &str,
    only_id: Option<&str>,
) -> Result<(), ControlError> {
    let ids = if let Some(id) = only_id {
        if store::with_db(|conn| store::reply_continuation_is_pending(conn, recipient, id))
            .map_err(control_error)?
        {
            vec![id.to_owned()]
        } else {
            Vec::new()
        }
    } else {
        store::with_db(|conn| store::pending_reply_continuations(conn, recipient))
            .map_err(control_error)?
    };
    for id in ids {
        let origin = store::with_db(|conn| store::requester_origin(conn, &id));
        let origin = match origin {
            Ok(Some(origin)) => origin,
            Ok(None) | Err(_) => {
                state
                    .interactions
                    .set_agent_reply_continuation_state(&id, "blocked_legacy_uncertain")
                    .await
                    .map_err(control_error)?;
                continue;
            }
        };

        let Some(lifecycle) = state.try_lock_agent_lifecycle(recipient).await else {
            state
                .interactions
                .set_agent_reply_continuation_state(&id, "waiting_busy")
                .await
                .map_err(control_error)?;
            continue;
        };
        let info = match delivery_target_info(state, recipient).await {
            Ok(info) => info,
            Err(_) => {
                state
                    .interactions
                    .set_agent_reply_continuation_state(&id, "waiting_identity")
                    .await
                    .map_err(control_error)?;
                continue;
            }
        };
        match durable_origin_matches(&origin, &info.provider, info.resume_session.as_deref()) {
            DurableOriginMatch::Current => {}
            DurableOriginMatch::WaitingForIdentity => {
                state
                    .interactions
                    .set_agent_reply_continuation_state(&id, "waiting_identity")
                    .await
                    .map_err(control_error)?;
                continue;
            }
            DurableOriginMatch::Stale => {
                state
                    .interactions
                    .set_agent_reply_continuation_state(&id, "blocked_stale_origin")
                    .await
                    .map_err(control_error)?;
                continue;
            }
        }
        if info.config.is_off || info.status == "off" {
            state
                .interactions
                .set_agent_reply_continuation_state(&id, "waiting_paused")
                .await
                .map_err(control_error)?;
            continue;
        }
        if active_conversation_lease_for_delivery(&info) {
            state
                .interactions
                .set_agent_reply_continuation_state(&id, "waiting_busy")
                .await
                .map_err(control_error)?;
            continue;
        }
        let Some(generation) = state
            .interactions
            .current_provider_input_generation(recipient)
            .await
            .filter(|generation| *generation > 0)
        else {
            state
                .interactions
                .set_agent_reply_continuation_state(&id, "waiting_identity")
                .await
                .map_err(control_error)?;
            continue;
        };

        let idle = wardian_core::identity::normalize_status(&info.status) == "idle";
        let native_route = match info.provider.as_str() {
            "codex" => {
                let selected = state
                    .native_delivery
                    .codex_owner_selected(recipient, generation)
                    .await;
                if selected {
                    match state
                        .native_delivery
                        .codex_binding(recipient, generation)
                        .await
                    {
                        Ok(binding) if binding.provider_session_id == origin.conversation_id => {
                            true
                        }
                        Ok(_) => {
                            state
                                .interactions
                                .set_agent_reply_continuation_state(&id, "blocked_stale_origin")
                                .await
                                .map_err(control_error)?;
                            continue;
                        }
                        Err(_) => {
                            state
                                .interactions
                                .set_agent_reply_continuation_state(&id, "waiting_identity")
                                .await
                                .map_err(control_error)?;
                            continue;
                        }
                    }
                } else {
                    false
                }
            }
            "pi" if idle => {
                let prepared = state
                    .native_delivery
                    .pi_bridge_prepared(recipient, generation)
                    .await;
                if prepared {
                    match state
                        .native_delivery
                        .pi_binding(recipient, generation)
                        .await
                    {
                        Ok(binding)
                            if binding.provider_session_id.as_deref()
                                == Some(origin.conversation_id.as_str()) =>
                        {
                            true
                        }
                        Ok(_) => {
                            state
                                .interactions
                                .set_agent_reply_continuation_state(&id, "blocked_stale_origin")
                                .await
                                .map_err(control_error)?;
                            continue;
                        }
                        Err(_) => {
                            state
                                .interactions
                                .set_agent_reply_continuation_state(&id, "waiting_busy")
                                .await
                                .map_err(control_error)?;
                            continue;
                        }
                    }
                } else {
                    false
                }
            }
            "opencode" if idle => {
                match state
                    .native_delivery
                    .opencode_http_eligibility(recipient, generation)
                    .await
                {
                    crate::delivery::native_broker::OpenCodeHttpEligibility::Unsupported => false,
                    crate::delivery::native_broker::OpenCodeHttpEligibility::Pending
                    | crate::delivery::native_broker::OpenCodeHttpEligibility::Failed => {
                        state
                            .interactions
                            .set_agent_reply_continuation_state(&id, "waiting_busy")
                            .await
                            .map_err(control_error)?;
                        continue;
                    }
                    crate::delivery::native_broker::OpenCodeHttpEligibility::Ready => {
                        if state
                            .native_delivery
                            .opencode_http_admission(recipient, generation, &info.config, &info.cwd)
                            .await
                            .is_ok()
                        {
                            true
                        } else {
                            state
                                .interactions
                                .set_agent_reply_continuation_state(&id, "waiting_busy")
                                .await
                                .map_err(control_error)?;
                            continue;
                        }
                    }
                }
            }
            _ => false,
        };
        if !idle && !(info.provider == "codex" && native_route) {
            state
                .interactions
                .set_agent_reply_continuation_state(&id, "waiting_busy")
                .await
                .map_err(control_error)?;
            continue;
        }
        if !native_route && provider_input_blocks_task_dispatch(state, recipient).await {
            state
                .interactions
                .set_agent_reply_continuation_state(&id, "waiting_busy")
                .await
                .map_err(control_error)?;
            continue;
        }
        let surface_runtime_generation = if !native_route {
            match state.terminal_sessions.broker_state(recipient).await {
                Ok(broker_state) => Some(broker_state.runtime_generation),
                Err(_) => {
                    state
                        .interactions
                        .set_agent_reply_continuation_state(&id, "waiting_identity")
                        .await
                        .map_err(control_error)?;
                    continue;
                }
            }
        } else {
            None
        };
        if !native_route
            && !surface_has_dedicated_turn_receipt(&info.provider)
            && !state
                .terminal_sessions
                .native_write_receipts_enabled(recipient)
                .await
                .unwrap_or(false)
        {
            state
                .interactions
                .set_agent_reply_continuation_state(&id, "waiting_identity")
                .await
                .map_err(control_error)?;
            continue;
        }
        state
            .interactions
            .set_agent_reply_continuation_state(&id, "pending")
            .await
            .map_err(control_error)?;
        let Some(claim) = state
            .interactions
            .claim_agent_reply_continuation(recipient, &id, generation)
            .await
            .map_err(control_error)?
        else {
            continue;
        };
        if info.provider == "codex" {
            // Codex has an existing context-only injection route. Drain it
            // before steering or starting the actionable reply turn.
            if let Err(error) = native::push_pending_information(state, recipient).await {
                manager::log_debug(&format!(
                    "[WARDIAN] pre-reply information delivery: {error}"
                ));
            }
        }
        let (context, information_claims) = prepare_claim_context_with_information(
            state,
            &claim,
            recipient,
            generation,
            &info.provider,
        )
        .await?;
        if native_route {
            drop(lifecycle);
            let result = native::dispatch_attached_reply(state, &info, &claim, &context).await;
            let outcome = match &result {
                Ok(state) if state == "failed_before_submit" => Err(false),
                Ok(state) => Ok(state.as_str()),
                Err(error) => Err(error.provider_boundary_crossed),
            };
            let delivery = native::settle(state, &claim, outcome).await;
            let information = settle_information_claims(state, information_claims, outcome).await;
            delivery.and(information)?;
            if let Err(error) = result {
                manager::log_debug(&format!(
                    "[WARDIAN] reply {} continuation: {}",
                    claim.record.id, error.message
                ));
            }
        } else {
            drop(lifecycle);
            let frame: wardian_core::agent_messaging::AgentMessageContext =
                serde_json::from_str(&context).map_err(|error| {
                    ControlError::request_failed(format!("reply context was invalid: {error}"))
                })?;
            let prompt = wardian_core::agent_messaging::reply_continuation_prompt(&frame);
            let result = crate::delivery::submit_live_surface_prompt(
                app,
                state,
                crate::delivery::LiveSurfacePromptRequest {
                    session_id: recipient.to_owned(),
                    prompt,
                    interaction_id: Some(claim.record.id.clone()),
                    input_mode: MessageInputMode::Message,
                    queue_policy: QueuePolicy::LiveOnly,
                    approval_action: None,
                    origin: claim
                        .record
                        .sender_session_id
                        .clone()
                        .map(|session_id| MessageOrigin::WardianAgent { session_id }),
                    write_fence: Some(
                        crate::utils::delivery_transaction::AutomatedInputWriteFence {
                            provider: info.provider.clone(),
                            provider_conversation_id: origin.conversation_id.clone(),
                            provider_input_generation: generation,
                            terminal_runtime_generation: surface_runtime_generation
                                .expect("surface route captured its terminal generation"),
                            agent_config_identity: info.config_identity.clone(),
                        },
                    ),
                    runtime_state: "agent_messaging_reply_continuation",
                    mark_prompt_started: true,
                    require_provider_turn_receipt: true,
                    payload_sent_detail: None,
                    delivery_message_id: Some(claim.record.id.clone()),
                },
            )
            .await;
            let outcome = match &result {
                Ok(_) => Ok("provider_visible"),
                Err(error) if error.retry_safe => Err(false),
                Err(_) => Err(true),
            };
            let delivery = native::settle(state, &claim, outcome).await;
            let information = settle_information_claims(state, information_claims, outcome).await;
            delivery.and(information)?;
            if let Err(error) = result {
                manager::log_debug(&format!(
                    "[WARDIAN] reply {} surface continuation: {}",
                    claim.record.id, error.message
                ));
            }
        }
    }
    Ok(())
}

pub(crate) async fn admit_automation_information(
    state: &AppState,
    run_id: &str,
    node: &str,
    recipient: &str,
    message: &str,
) -> Result<store::Admitted, ControlError> {
    let id = resolve_automation_recipient(state, recipient).await?;
    if id != recipient {
        return Err(ControlError::not_found(
            "Resolved automation recipient no longer exists.",
        ));
    }
    state
        .interactions
        .admit_host_automation_message(run_id, node, &id, message)
        .await
        .map_err(control_error)
}

struct AdmissionInput<'a> {
    sender: &'a str,
    target: &'a str,
    message: &'a str,
    key: Option<&'a str>,
    task: bool,
}

/// Capture the live managed configuration's persisted provider conversation ID.
/// This deliberately avoids archive hydration, so logging-disabled agents can
/// still bind replies to the conversation their provider process is resuming.
async fn requester_origin_for_admission(
    state: &AppState,
    sender: &str,
) -> Option<store::RequesterOrigin> {
    let config_handle = {
        let agents = state.agents.lock().await;
        agents.get(sender).map(|agent| agent.config.clone())?
    };
    let config = config_handle.lock().ok()?.clone();
    let provider = config.provider.trim().to_ascii_lowercase();
    let conversation_id = config
        .resume_session
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let generation = state
        .interactions
        .current_provider_input_generation(sender)
        .await?;
    (generation > 0 && !provider.is_empty()).then_some(store::RequesterOrigin {
        provider,
        conversation_id,
        generation,
    })
}

/// Capture the recipient's persisted provider conversation under its lifecycle
/// gate. No archive hydration or logging configuration is involved.
async fn recipient_origin_for_admission(
    state: &AppState,
    recipient: &str,
) -> Option<store::RecipientOrigin> {
    let config_handle = {
        let agents = state.agents.lock().await;
        agents.get(recipient).map(|agent| agent.config.clone())?
    };
    let config = config_handle.lock().ok()?.clone();
    let provider = config.provider.trim().to_ascii_lowercase();
    let conversation_id = config
        .resume_session
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let generation = state
        .interactions
        .current_provider_input_generation(recipient)
        .await?;
    (generation > 0 && !provider.is_empty()).then_some(store::RecipientOrigin {
        provider,
        conversation_id,
        generation,
    })
}

async fn admit(
    app: Option<&AppHandle>,
    state: &AppState,
    input: AdmissionInput<'_>,
) -> Result<Response, ControlError> {
    let AdmissionInput {
        sender,
        target,
        message,
        key,
        task,
    } = input;
    store::validate_message(message).map_err(control_error)?;
    let resolved = if target.starts_with("controller:") {
        if task {
            return Err(ControlError::coded(
                "invalid_target",
                "Controllers are receive-only mailboxes, never task workers.",
            ));
        }
        wardian_core::controllers::ControllerStore::current()
            .and_then(|store| store.authorize_mailbox_sender(target, sender))
            .map_err(|_| {
                ControlError::coded(
                    "forbidden",
                    "Controller mailbox does not accept information from this sender.",
                )
            })?;
        ResolvedRecipient {
            id: target.into(),
            provider: WorkerProviderClassification::Unknown,
        }
    } else {
        resolve_exact(state, target).await?
    };
    let recipient = resolved.id;
    // New Session/clear operations use this same per-agent lifecycle gate.
    // Hold it from the durable identity snapshot through canonical admission.
    let sender_lifecycle = if task {
        Some(state.lock_agent_lifecycle(sender).await)
    } else {
        None
    };
    let recipient_lifecycle = if task {
        None
    } else {
        Some(state.lock_agent_lifecycle(&recipient).await)
    };
    let requester_origin = if task {
        requester_origin_for_admission(state, sender).await
    } else {
        None
    };
    let recipient_origin = if task {
        None
    } else {
        recipient_origin_for_admission(state, &recipient).await
    };
    let generation = state
        .interactions
        .current_provider_input_generation(&recipient)
        .await
        .unwrap_or(0);
    let admission = store::Admission {
        sender,
        recipient: &recipient,
        message,
        idempotency_key: key,
        task,
        generation,
    };
    let admitted = if task {
        state
            .interactions
            .admit_agent_message_with_requester_origin(admission, requester_origin.as_ref())
            .await
    } else {
        state
            .interactions
            .admit_agent_message_with_wake_on_admission(admission, recipient_origin.as_ref())
            .await
    }
    .map_err(control_error)?;
    drop(recipient_lifecycle);
    drop(sender_lifecycle);
    if let Some(app) = app {
        if task {
            let _ = app.emit("pair-activity-changed", ());
        }
        if task && !admitted.duplicate {
            spawn_pending_tasks_with_request(
                app,
                &recipient,
                Some(admitted.record.id.clone()),
                Some(generation),
                Some(resolved.provider),
            );
        } else if !task {
            notify_information_admitted(app, &recipient, &admitted);
        }
    }
    if task {
        Ok(Response::FollowupTask {
            request_id: admitted.record.id,
            delivery_state: admitted.delivery_state,
            delivery_owner: admitted.owner,
            duplicate: admitted.duplicate,
        })
    } else {
        Ok(Response::SendMessage {
            interaction_id: admitted.record.id,
            delivery_state: admitted.delivery_state,
            duplicate: admitted.duplicate,
        })
    }
}

/// Idle status observations and new admissions give unclaimed work a chance to
/// run. Received or uncertain work is excluded by the durable claim transaction.
pub(crate) fn spawn_pending_tasks(app: &AppHandle, recipient: &str) {
    spawn_pending_tasks_with_request(app, recipient, None, None, None);
}

/// Queue a status or startup-readiness opportunity after roster contention has
/// cleared. The attachment gate is checked from an Arc snapshot outside the
/// roster lock, then the agent incarnation is revalidated before dispatch.
pub(super) fn spawn_pending_tasks_when_ready(
    app: &AppHandle,
    recipient: &str,
    expected_status: Option<&str>,
) {
    let expected_status = expected_status.map(wardian_core::identity::normalize_status);
    if expected_status
        .as_deref()
        .is_some_and(|status| !matches!(status, "idle" | "off"))
    {
        return;
    }

    let app = app.clone();
    let recipient = recipient.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if !pending_task_wakeup_is_ready(&state, &recipient, expected_status.as_deref()).await {
            return;
        }
        spawn_pending_tasks(&app, &recipient);
    });
}

async fn pending_task_wakeup_is_ready(
    state: &AppState,
    recipient: &str,
    expected_status: Option<&str>,
) -> bool {
    let Some(snapshot) = pending_task_wakeup_agent_snapshot(state, recipient).await else {
        return false;
    };
    pending_task_wakeup_snapshot_is_ready_and_current(state, recipient, &snapshot, expected_status)
        .await
}

struct PendingTaskWakeAgentSnapshot {
    current_status: std::sync::Arc<std::sync::Mutex<String>>,
    config: std::sync::Arc<std::sync::Mutex<wardian_core::models::AgentConfig>>,
    watch_state: std::sync::Arc<std::sync::Mutex<crate::state::AgentWatchState>>,
    runtime_generation: Option<u64>,
}

async fn pending_task_wakeup_agent_snapshot(
    state: &AppState,
    recipient: &str,
) -> Option<PendingTaskWakeAgentSnapshot> {
    let agents = state.agents.lock().await;
    let agent = agents.get(recipient)?;
    Some(PendingTaskWakeAgentSnapshot {
        current_status: agent.current_status.clone(),
        config: agent.config.clone(),
        watch_state: agent.watch_state.clone(),
        runtime_generation: agent.runtime_generation,
    })
}

fn pending_task_wakeup_snapshot_is_ready(
    snapshot: &PendingTaskWakeAgentSnapshot,
    expected_status: Option<&str>,
) -> bool {
    let status_ready = snapshot
        .current_status
        .lock()
        .map(|status| {
            expected_status.is_none_or(|expected| {
                wardian_core::identity::normalize_status(&status) == expected
            })
        })
        .unwrap_or(false);
    if !status_ready {
        return false;
    }

    let Ok(config) = snapshot.config.lock() else {
        return false;
    };
    let codex_agent = config.provider == "codex";
    drop(config);
    !codex_agent
        || snapshot
            .watch_state
            .lock()
            .is_ok_and(|watch_state| watch_state.codex_attachment_ready())
}

async fn pending_task_wakeup_snapshot_is_current(
    state: &AppState,
    recipient: &str,
    snapshot: &PendingTaskWakeAgentSnapshot,
) -> bool {
    let agents = state.agents.lock().await;
    agents.get(recipient).is_some_and(|current| {
        current.runtime_generation == snapshot.runtime_generation
            && std::sync::Arc::ptr_eq(&current.current_status, &snapshot.current_status)
            && std::sync::Arc::ptr_eq(&current.config, &snapshot.config)
            && std::sync::Arc::ptr_eq(&current.watch_state, &snapshot.watch_state)
    })
}

async fn pending_task_wakeup_snapshot_is_ready_and_current(
    state: &AppState,
    recipient: &str,
    snapshot: &PendingTaskWakeAgentSnapshot,
    expected_status: Option<&str>,
) -> bool {
    pending_task_wakeup_snapshot_is_ready(snapshot, expected_status)
        && pending_task_wakeup_snapshot_is_current(state, recipient, snapshot).await
}

pub(super) async fn lock_agent_lifecycle_for_task_dispatch(
    state: &AppState,
    info: &DeliveryTargetInfo,
    request_id: Option<&str>,
) -> tokio::sync::OwnedMutexGuard<()> {
    let lifecycle_lock = state.agent_lifecycle_lock_for(&info.uuid).await;
    match lifecycle_lock.clone().try_lock_owned() {
        Ok(guard) => guard,
        Err(_) => {
            // Lifecycle ownership commonly overlaps a short startup handoff.
            // Wait briefly before recording a watch event so transient lock
            // contention does not leave a misleading pending observation.
            let watch_delay = tokio::time::sleep(std::time::Duration::from_secs(1));
            tokio::pin!(watch_delay);
            let mut published_wait = false;
            let mut diagnostics = tokio::time::interval(std::time::Duration::from_secs(30));
            diagnostics.tick().await;
            let lock = lifecycle_lock.lock_owned();
            tokio::pin!(lock);
            loop {
                tokio::select! {
                    guard = &mut lock => return guard,
                    _ = &mut watch_delay, if !published_wait => {
                        publish_lifecycle_busy_delivery(state, info, request_id).await;
                        diagnose_lifecycle_wait(state, info, request_id).await;
                        published_wait = true;
                    }
                    _ = diagnostics.tick() => {
                        if published_wait {
                            diagnose_lifecycle_wait(state, info, request_id).await;
                        }
                    },
                }
            }
        }
    }
}

async fn publish_lifecycle_busy_delivery(
    state: &AppState,
    info: &DeliveryTargetInfo,
    request_id: Option<&str>,
) {
    let detail = wardian_core::control::DeliveryDetail {
        uuid: info.uuid.clone(),
        name: info.name.clone(),
        provider: info.provider.clone(),
        runtime_state: "lifecycle_busy".to_string(),
        delivery_state: "pending".to_string(),
        input_mode: wardian_core::control::MessageInputMode::Message,
        queue_policy: wardian_core::control::QueuePolicy::QueueIfBusy,
        message_id: request_id.map(|request_id| request_id.to_string()),
        delivery_phase: Some("preclaim".to_string()),
        observed_state: Some("lifecycle_busy".to_string()),
        reason: Some("lifecycle_busy".to_string()),
        profile: None,
        error: None,
    };
    crate::control::push_delivery_for_delivery_service(state, &info.uuid, &detail).await;
}

async fn diagnose_lifecycle_wait(
    state: &AppState,
    info: &DeliveryTargetInfo,
    request_id: Option<&str>,
) {
    if info.provider == "opencode" && request_id.is_some() {
        log_opencode_dispatch_stage(
            state,
            info,
            request_id,
            OpenCodeDispatchDiagnosticStage::Preclaim,
            OpenCodeDispatchDiagnosticReason::LifecycleBusy,
        )
        .await;
    } else {
        manager::log_debug(&format!(
            "[WARDIAN] v2 task dispatch waiting for lifecycle: session={} provider={} reason=lifecycle_busy",
            info.uuid, info.provider
        ));
    }
}

fn spawn_pending_tasks_with_request(
    app: &AppHandle,
    recipient: &str,
    request_id: Option<String>,
    generation: Option<u64>,
    provider: Option<WorkerProviderClassification>,
) {
    // Ready/idle observations also give stored information a push opportunity;
    // this path itself can only inspect an already-existing native owner.
    native::spawn_information(app, recipient);
    let app = app.clone();
    let recipient = recipient.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = dispatch_pending_queue_with_request(
            Some(&app),
            &app.state::<AppState>(),
            &recipient,
            request_id.as_deref(),
            generation,
            provider,
        )
        .await
        {
            manager::log_debug(&format!("[WARDIAN] v2 task dispatch: {error}"));
        }
    });
}

/// After a completed dispatch returns, its lease/execution guard has dropped.
/// A changed queue head is distinct admitted work. An unchanged head means busy
/// or unsupported and stops this opportunity without polling or replay.
pub(super) async fn dispatch_pending_queue(
    app: Option<&AppHandle>,
    state: &AppState,
    recipient: &str,
) -> Result<(), ControlError> {
    dispatch_pending_queue_with_request(app, state, recipient, None, None, None).await
}

#[derive(Debug)]
struct OpenCodeDispatchWorkerEntry {
    request_id: String,
    generation: u64,
    provider: WorkerProviderClassification,
}

async fn dispatch_pending_queue_with_request(
    app: Option<&AppHandle>,
    state: &AppState,
    recipient: &str,
    request_id: Option<&str>,
    generation: Option<u64>,
    provider: Option<WorkerProviderClassification>,
) -> Result<(), ControlError> {
    let worker_entry =
        request_id
            .zip(generation)
            .map(|(request_id, generation)| OpenCodeDispatchWorkerEntry {
                request_id: request_id.to_string(),
                generation,
                provider: provider.unwrap_or(WorkerProviderClassification::Unknown),
            });
    let mut worker_entry = Some(worker_entry);
    loop {
        let entry = worker_entry.take().flatten();
        if let Some(entry) = entry.as_ref() {
            log_opencode_worker_entry(entry, OpenCodeDispatchDiagnosticReason::WorkerStarted);
        }
        let before = store::with_db(|conn| store::next_pending_task_id(conn, recipient))
            .map_err(control_error)?;
        if let Some(entry) = entry.as_ref() {
            let reason = match before.as_deref() {
                Some(id) if id == entry.request_id => {
                    OpenCodeDispatchDiagnosticReason::QueueHeadMatches
                }
                Some(_) => OpenCodeDispatchDiagnosticReason::QueueHeadOther,
                None => OpenCodeDispatchDiagnosticReason::QueueHeadEmpty,
            };
            log_opencode_worker_entry(entry, reason);
        }
        if before.is_none() {
            return Ok(());
        }
        let outcome = dispatch_one_with_request(app, state, recipient, before.as_deref()).await;
        let after = store::with_db(|conn| store::next_pending_task_id(conn, recipient))
            .map_err(control_error)?;
        if let Some(entry) = entry.as_ref() {
            log_opencode_worker_entry(
                entry,
                if after == before {
                    OpenCodeDispatchDiagnosticReason::QueueHeadUnchanged
                } else {
                    OpenCodeDispatchDiagnosticReason::QueueHeadAdvanced
                },
            );
        }
        if after == before {
            return outcome;
        }
        if let Err(error) = outcome {
            manager::log_debug(&format!(
                "[WARDIAN] completed queue ownership with delivery error: {error}"
            ));
        }
    }
}

fn log_opencode_worker_entry(
    entry: &OpenCodeDispatchWorkerEntry,
    reason: OpenCodeDispatchDiagnosticReason,
) {
    let reason = match entry.provider {
        WorkerProviderClassification::OpenCode => reason,
        WorkerProviderClassification::Other => return,
        WorkerProviderClassification::Unknown => {
            if reason != OpenCodeDispatchDiagnosticReason::WorkerStarted {
                return;
            }
            OpenCodeDispatchDiagnosticReason::WorkerProviderUnknown
        }
    };
    log_opencode_dispatch_diagnostic(
        &entry.request_id,
        entry.generation,
        OpenCodeDispatchDiagnosticStage::WorkerEntry,
        reason,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskDispatchRoute {
    Native,
    Background,
    Surface,
}

fn task_dispatch_route(
    provider: &str,
    explicit_off: bool,
    status: &str,
    selected_native_owner: bool,
) -> TaskDispatchRoute {
    if explicit_off {
        TaskDispatchRoute::Background
    } else if selected_native_owner {
        TaskDispatchRoute::Native
    } else if status_uses_headless_delivery(status) {
        TaskDispatchRoute::Background
    } else if provider == "codex" {
        TaskDispatchRoute::Native
    } else {
        TaskDispatchRoute::Surface
    }
}

async fn dispatch_one_with_request(
    app: Option<&AppHandle>,
    state: &AppState,
    recipient: &str,
    request_id: Option<&str>,
) -> Result<(), ControlError> {
    let info = delivery_target_info(state, recipient).await?;
    // A background owner keeps the conversation out of every competing route.
    if active_conversation_lease_for_delivery(&info) {
        log_opencode_dispatch_stage(
            state,
            &info,
            request_id,
            OpenCodeDispatchDiagnosticStage::Preclaim,
            OpenCodeDispatchDiagnosticReason::ConversationLease,
        )
        .await;
        return Ok(());
    }
    let selected_native_owner = native::selected_native_owner_for_dispatch(state, &info).await;
    let route = task_dispatch_route(
        &info.provider,
        info.config.is_off,
        &info.status,
        selected_native_owner,
    );
    let route_reason = match route {
        TaskDispatchRoute::Native => OpenCodeDispatchDiagnosticReason::RouteNative,
        TaskDispatchRoute::Background => OpenCodeDispatchDiagnosticReason::RouteBackground,
        TaskDispatchRoute::Surface => OpenCodeDispatchDiagnosticReason::RouteSurface,
    };
    log_opencode_dispatch_stage(
        state,
        &info,
        request_id,
        OpenCodeDispatchDiagnosticStage::Preclaim,
        route_reason,
    )
    .await;
    match route {
        TaskDispatchRoute::Native => {
            return native::dispatch_attached_task_with_request(app, state, &info, request_id).await
        }
        TaskDispatchRoute::Background => return dispatch_background_task(app, state, &info).await,
        TaskDispatchRoute::Surface => {}
    }
    // This is a detached queue worker, so wait for startup/lifecycle ownership
    // to release instead of returning pending and relying on a later event to
    // retry. Revalidate the incarnation and readiness after the wait.
    let _lifecycle = lock_agent_lifecycle_for_task_dispatch(state, &info, request_id).await;
    let current = delivery_target_info(state, recipient).await?;
    if !same_delivery_target_incarnation(&info, &current)
        || info.provider != current.provider
        || info.config.is_off != current.config.is_off
    {
        return Ok(());
    }
    let info = current;
    if info.status != "idle" {
        log_opencode_dispatch_stage(
            state,
            &info,
            request_id,
            OpenCodeDispatchDiagnosticStage::Preclaim,
            OpenCodeDispatchDiagnosticReason::ProviderNotReady,
        )
        .await;
        return Ok(());
    }
    if provider_input_blocks_task_dispatch(state, recipient).await {
        log_opencode_dispatch_stage(
            state,
            &info,
            request_id,
            OpenCodeDispatchDiagnosticStage::Preclaim,
            OpenCodeDispatchDiagnosticReason::ProviderInputBusy,
        )
        .await;
        return Ok(());
    }
    if active_conversation_lease_for_delivery(&info) {
        log_opencode_dispatch_stage(
            state,
            &info,
            request_id,
            OpenCodeDispatchDiagnosticStage::Preclaim,
            OpenCodeDispatchDiagnosticReason::ConversationLease,
        )
        .await;
        return Ok(());
    }
    if state
        .terminal_sessions
        .broker_state(recipient)
        .await
        .is_err()
    {
        log_opencode_dispatch_stage(
            state,
            &info,
            request_id,
            OpenCodeDispatchDiagnosticStage::Preclaim,
            OpenCodeDispatchDiagnosticReason::BrokerUnavailable,
        )
        .await;
        return Ok(());
    }
    let generation = state
        .interactions
        .current_provider_input_generation(recipient)
        .await
        .unwrap_or(0);
    let Some(claim) = state
        .interactions
        .claim_agent_task(recipient, generation)
        .await
        .map_err(control_error)?
    else {
        log_opencode_dispatch_diagnostic_for_info(
            &info,
            request_id,
            generation,
            OpenCodeDispatchDiagnosticStage::Preclaim,
            OpenCodeDispatchDiagnosticReason::NoClaim,
        );
        return Ok(());
    };
    if info.provider == "opencode" {
        log_opencode_dispatch_diagnostic(
            &claim.record.id,
            generation,
            OpenCodeDispatchDiagnosticStage::PostclaimPreNativeAdmission,
            OpenCodeDispatchDiagnosticReason::ClaimAcquired,
        );
    }
    let (context, information_claims) = prepare_claim_context_with_information(
        state,
        &claim,
        recipient,
        generation,
        &info.provider,
    )
    .await?;
    let frame: wardian_core::agent_messaging::AgentMessageContext = serde_json::from_str(&context)
        .map_err(|error| ControlError::request_failed(error.to_string()))?;
    let information = wardian_core::agent_messaging::pending_information_prompt(&frame);
    let prompt = if information.is_empty() {
        message_with_structured_reply_instruction(&context, &claim.record.id)
    } else {
        format!(
            "{information}\n\n{}",
            message_with_structured_reply_instruction(&context, &claim.record.id)
        )
    };
    let result = crate::delivery::submit_live_surface_prompt(
        app,
        state,
        crate::delivery::LiveSurfacePromptRequest {
            session_id: recipient.into(),
            prompt,
            interaction_id: Some(claim.record.id.clone()),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::LiveOnly,
            approval_action: None,
            origin: claim
                .record
                .sender_session_id
                .clone()
                .map(|session_id| MessageOrigin::WardianAgent { session_id }),
            write_fence: None,
            runtime_state: "agent_messaging_task",
            mark_prompt_started: true,
            require_provider_turn_receipt: true,
            payload_sent_detail: None,
            delivery_message_id: Some(claim.record.id.clone()),
        },
    )
    .await;
    if info.provider == "opencode" {
        log_opencode_dispatch_diagnostic(
            &claim.record.id,
            generation,
            OpenCodeDispatchDiagnosticStage::ActualDispatch,
            OpenCodeDispatchDiagnosticReason::SurfaceSubmit,
        );
    }
    let outcome = match &result {
        Ok(_) => "provider_visible",
        Err(error) if error.retry_safe => "failed_before_submit",
        Err(_) => "uncertain",
    };
    let delivery = state
        .interactions
        .finish_agent_task(&claim, outcome)
        .await
        .map_err(control_error);
    let information = settle_information_claims(
        state,
        information_claims,
        match &result {
            Ok(_) => Ok("provider_visible"),
            Err(error) if error.retry_safe => Err(false),
            Err(_) => Err(true),
        },
    )
    .await;
    delivery.and(information)?;
    Ok(())
}

async fn log_opencode_dispatch_stage(
    state: &AppState,
    info: &DeliveryTargetInfo,
    request_id: Option<&str>,
    stage: OpenCodeDispatchDiagnosticStage,
    reason: OpenCodeDispatchDiagnosticReason,
) {
    let Some(request_id) = request_id.filter(|_| info.provider == "opencode") else {
        return;
    };
    let generation = state
        .interactions
        .current_provider_input_generation(&info.uuid)
        .await
        .unwrap_or(0);
    log_opencode_dispatch_diagnostic(request_id, generation, stage, reason);
}

fn log_opencode_dispatch_diagnostic_for_info(
    info: &DeliveryTargetInfo,
    request_id: Option<&str>,
    generation: u64,
    stage: OpenCodeDispatchDiagnosticStage,
    reason: OpenCodeDispatchDiagnosticReason,
) {
    if info.provider == "opencode" {
        if let Some(request_id) = request_id {
            log_opencode_dispatch_diagnostic(request_id, generation, stage, reason);
        }
    }
}

/// The existing off-agent policy permits explicit work to run headlessly.
/// Acquire the process guard and persisted conversation lease before the local
/// lifecycle gate, then claim canonical work at the actual execution boundary.
async fn dispatch_background_task(
    app: Option<&AppHandle>,
    state: &AppState,
    info: &DeliveryTargetInfo,
) -> Result<(), ControlError> {
    let has_pending = store::with_db(|conn| Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_message_delivery d JOIN interactions i ON i.id=d.interaction_id WHERE d.recipient=?1 AND d.owner='pending' AND d.operation='followup_task' AND i.status='awaiting_reply')",
        [&info.uuid], |row| row.get::<_,bool>(0))?)).map_err(control_error)?;
    if !has_pending || active_conversation_lease_for_delivery(info) {
        return Ok(());
    }
    let _execution = wardian_core::automation_execution_lock::acquire_headless_execution_guard()
        .map_err(|error| ControlError::request_failed(error.to_string()))?;
    let owner = format!("agent_messaging_{}", uuid::Uuid::new_v4().simple());
    let lease = match acquire_headless_message_lease(info, &owner) {
        Ok(lease) => lease,
        Err(HeadlessMessageLeaseError::Busy) => return Ok(()),
        Err(HeadlessMessageLeaseError::Failed(error)) => {
            return Err(ControlError::request_failed(error))
        }
    };
    let mut lease_guard =
        wardian_core::conversation_lease::PersistedConversationLeaseGuard::new(&lease);
    let Some(lifecycle) = state.try_lock_agent_lifecycle(&info.uuid).await else {
        return Ok(());
    };
    let current = delivery_target_info(state, &info.uuid).await?;
    if !same_delivery_target_incarnation(info, &current)
        || !status_uses_headless_delivery(&current.status)
    {
        return Ok(());
    }
    // Each background Codex process owns a new input generation. Reserve it
    // under the short lifecycle guard before the durable task claim, so a late
    // cleanup from the preceding process cannot dispose this replacement.
    let generation = if current.provider == "codex" {
        state
            .interactions
            .start_provider_input_generation(&info.uuid, ProviderInputReadiness::Booting, None)
            .await
            .generation
    } else {
        state
            .interactions
            .current_provider_input_generation(&info.uuid)
            .await
            .unwrap_or(0)
    };
    let Some(claim) = state
        .interactions
        .claim_agent_task(&info.uuid, generation)
        .await
        .map_err(control_error)?
    else {
        return Ok(());
    };
    let (context, information_claims) = prepare_claim_context_with_information(
        state,
        &claim,
        &info.uuid,
        generation,
        &current.provider,
    )
    .await?;
    let frame: wardian_core::agent_messaging::AgentMessageContext = serde_json::from_str(&context)
        .map_err(|error| ControlError::request_failed(error.to_string()))?;
    let information = wardian_core::agent_messaging::pending_information_prompt(&frame);
    let prompt = if information.is_empty() {
        message_with_structured_reply_instruction(&context, &claim.record.id)
    } else {
        format!(
            "{information}\n\n{}",
            message_with_structured_reply_instruction(&context, &claim.record.id)
        )
    };
    record_headless_status_observation(app, state, &current).await;
    drop(lifecycle);
    if current.provider == "codex" {
        return native::dispatch_background(app, state, &current, claim, lease_guard).await;
    }
    // The persisted lease owns the conversation through child exit and cleanup;
    // no lifecycle/global roster lock is held while the provider runs.
    let result = crate::delivery::run_headless_process_prompt(
        state,
        crate::delivery::HeadlessProcessPromptRequest {
            node: "agent_messaging_task".into(),
            provider: current.provider.clone(),
            cwd: current.cwd.clone(),
            prompt: prompt.clone(),
            session_id: current.uuid.clone(),
            memory_agent_id: Some(current.uuid.clone()),
            resume_session: current.resume_session.clone(),
            config_override: Some(current.config.clone()),
            interaction_id: Some(claim.record.id.clone()),
            timeout: bounded_headless_delivery_timeout(None),
            lease_owner: Some(lease_guard.owner().clone()),
            cancellation_marker: None,
        },
    )
    .await;
    let outcome = if result.is_ok() {
        "provider_visible"
    } else {
        "uncertain"
    };
    let delivery = state
        .interactions
        .finish_agent_task(&claim, outcome)
        .await
        .map_err(control_error);
    let information = settle_information_claims(
        state,
        information_claims,
        if result.is_ok() {
            Ok("provider_visible")
        } else {
            Err(true)
        },
    )
    .await;
    delivery.and(information)?;
    if let Ok(result) = result {
        record_headless_message_response(state, &current, &claim.record.id, &result.response).await;
        let origin = claim
            .record
            .sender_session_id
            .clone()
            .map(|session_id| MessageOrigin::WardianAgent { session_id });
        record_headless_message_exchange(
            state,
            &current,
            &claim.record.id,
            &prompt,
            &result.response,
            origin.as_ref(),
        )
        .await;
    }
    lease_guard
        .release()
        .map_err(ControlError::request_failed)?;
    record_headless_status_observation(app, state, &current).await;
    Ok(())
}

/// Framing precedes provider I/O. Failure explicitly releases the exact claim
/// rather than leaving a message permanently owned without crossing a boundary.
async fn prepare_claim_context(
    state: &AppState,
    claim: &store::TaskClaim,
) -> Result<String, ControlError> {
    match context_frame(claim) {
        Ok(context) => Ok(context),
        Err(error) => {
            state
                .interactions
                .release_agent_message_before_write(claim)
                .await
                .map_err(control_error)?;
            Err(error)
        }
    }
}

#[derive(Default)]
struct PendingInformationClaims {
    claims: Vec<store::TaskClaim>,
    frames: Vec<wardian_core::agent_messaging::AgentMessageContext>,
}

/// Claim the bounded FYI prefix for the next managed non-Codex turn. Codex
/// retains its existing context-only push, which never starts a turn.
async fn prepare_claim_context_with_information(
    state: &AppState,
    claim: &store::TaskClaim,
    recipient: &str,
    generation: u64,
    provider: &str,
) -> Result<(String, PendingInformationClaims), ControlError> {
    let context = prepare_claim_context(state, claim).await?;
    let mut frame: wardian_core::agent_messaging::AgentMessageContext =
        match serde_json::from_str(&context) {
            Ok(frame) => frame,
            Err(error) => {
                state
                    .interactions
                    .release_agent_message_before_write(claim)
                    .await
                    .map_err(control_error)?;
                return Err(ControlError::request_failed(error.to_string()));
            }
        };
    let mut pending = PendingInformationClaims::default();
    let prepare_information = async {
        if !provider.eq_ignore_ascii_case("codex") {
            let highwater = store::with_db(|conn| store::information_highwater(conn, recipient))
                .map_err(control_error)?;
            let ids = store::with_db(|conn| store::pending_information(conn, recipient, highwater))
                .map_err(control_error)?;
            for id in ids {
                let information = match state
                    .interactions
                    .claim_agent_information(recipient, &id, generation)
                    .await
                {
                    Ok(Some(information)) => information,
                    Ok(None) => continue,
                    Err(error) => return Err(control_error(error)),
                };
                let information_frame =
                    store::with_db(|conn| store::message_context(conn, &information.record));
                match information_frame {
                    Ok(information_frame) => {
                        pending.claims.push(information);
                        pending.frames.push(information_frame);
                    }
                    Err(error) => {
                        state
                            .interactions
                            .release_agent_message_before_write(&information)
                            .await
                            .map_err(control_error)?;
                        return Err(control_error(error));
                    }
                }
            }
        }
        Ok::<(), ControlError>(())
    }
    .await;
    if let Err(error) = prepare_information {
        let information = settle_information_claims(state, pending, Err(false)).await;
        let delivery = state
            .interactions
            .release_agent_message_before_write(claim)
            .await
            .map_err(control_error);
        delivery.and(information)?;
        return Err(error);
    }
    frame.pending_information = pending.frames.clone();
    let context = match serde_json::to_string(&frame) {
        Ok(context) => context,
        Err(error) => {
            let information = settle_information_claims(state, pending, Err(false)).await;
            let delivery = state
                .interactions
                .release_agent_message_before_write(claim)
                .await
                .map_err(control_error);
            delivery.and(information)?;
            return Err(ControlError::request_failed(error.to_string()));
        }
    };
    Ok((context, pending))
}

async fn settle_information_claims(
    state: &AppState,
    claims: PendingInformationClaims,
    outcome: Result<&str, bool>,
) -> Result<(), ControlError> {
    let mut first_error = None;
    for claim in claims.claims {
        let result = match outcome {
            Ok(delivery_state) => state
                .interactions
                .finish_agent_task(&claim, delivery_state)
                .await
                .map_err(control_error),
            Err(false) => state
                .interactions
                .release_agent_message_before_write(&claim)
                .await
                .map_err(control_error),
            Err(true) => state
                .interactions
                .finish_agent_task(&claim, "uncertain")
                .await
                .map_err(control_error),
        };
        if first_error.is_none() {
            first_error = result.err();
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn context_frame(claim: &store::TaskClaim) -> Result<String, ControlError> {
    let context = store::with_db(|conn| store::message_context(conn, &claim.record))
        .map_err(control_error)?;
    serde_json::to_string(&context).map_err(|error| ControlError::request_failed(error.to_string()))
}

fn control_error(error: AgentMessagingError) -> ControlError {
    let code = match error.code.as_str() {
        "unauthorized" => "unauthorized",
        "not_found" => "not_found",
        "invalid_message" => "invalid_message",
        "invalid_limit" => "invalid_limit",
        "invalid_cursor" => "invalid_cursor",
        "expired_cursor" => "expired_cursor",
        "idempotency_conflict" => "idempotency_conflict",
        "invalid_idempotency_key" => "invalid_idempotency_key",
        "conflicting_reply" => "conflicting_reply",
        "stale_claim" => "stale_claim",
        _ => "storage_error",
    };
    ControlError::coded(code, error.message)
}

pub(crate) fn message_with_structured_reply_instruction(message: &str, request_id: &str) -> String {
    format!(
        "{message}\n\nWardian request id: {request_id}\nWhen finished, use the canonical reply tool with request_id {request_id}, status done, and message containing your reply, or run wardian message reply {request_id} --status done --stdin with the reply body on stdin. Use status blocked or failed if you cannot complete it. Printing a final answer alone does not record the structured reply."
    )
}

pub(super) fn bounded_headless_delivery_timeout(timeout_ms: Option<u64>) -> Duration {
    timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT)
        .max(Duration::from_secs(1))
        .min(MAX_HEADLESS_DELIVERY_TIMEOUT)
}

pub(super) struct HeadlessMessageDeliveryRequest<'a> {
    pub(super) app: Option<&'a AppHandle>,
    pub(super) info: &'a DeliveryTargetInfo,
    pub(super) interaction_id: &'a str,
    pub(super) prompt: &'a str,
    pub(super) input_mode: MessageInputMode,
    pub(super) queue_policy: QueuePolicy,
    pub(super) origin: Option<&'a MessageOrigin>,
    pub(super) timeout: Duration,
    pub(super) lifecycle_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    pub(super) orchestration: Option<&'a wardian_core::control::OrchestrationDeliveryOptions>,
    pub(super) parent_interaction_id: Option<&'a str>,
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod wake_tests;
