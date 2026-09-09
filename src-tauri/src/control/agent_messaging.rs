//! Receiver-first messaging admission and dispatch. Provider integrations must
//! acquire the same durable delivery claim before exposing canonical task text.
use super::*;
use wardian_core::agent_messaging::{
    AgentMessagingError, AgentMessagingRequest as Request, AgentMessagingResponse as Response,
    MAX_RECEIVE_ITEMS, MAX_RECEIVE_TIMEOUT_MS,
};
use wardian_core::db::agent_messaging as store;
mod native;

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
                native::spawn_information(app, &replied.record.target_session_ids[0]);
            }
            Ok(Response::Reply {
                request_id,
                interaction_id: replied.record.id,
                delivery_state: "stored".into(),
                duplicate: replied.duplicate,
            })
        }
        Request::ReceiveMessages {
            cursor,
            ack_cursor,
            limit,
            timeout_ms,
        } => {
            let limit = limit.unwrap_or(MAX_RECEIVE_ITEMS);
            let timeout = timeout_ms.unwrap_or(0);
            if limit == 0 || limit > MAX_RECEIVE_ITEMS || timeout > MAX_RECEIVE_TIMEOUT_MS {
                return Err(ControlError::coded(
                    "invalid_receive_bounds",
                    "Limit must be 1..100 and timeout_ms 0..60000.",
                ));
            }
            // Snapshot before the first read, so a native claim already in flight
            // cannot settle between an empty read and registration unnoticed.
            let provider_revision = state
                .interactions
                .agent_message_provider_revision(&sender)
                .await;
            let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
            loop {
                authenticate(state, &sender).await?;
                let mut page = state
                    .interactions
                    .receive_agent_messages(
                        &sender,
                        cursor.as_deref(),
                        ack_cursor.as_deref(),
                        limit,
                    )
                    .await
                    .map_err(control_error)?;
                if timeout > 0
                    && state
                        .interactions
                        .agent_message_provider_revision(&sender)
                        .await
                        != provider_revision
                {
                    page.wake_reason = Some("provider_context_available".into());
                }
                if !page.messages.is_empty() || page.wake_reason.is_some() || timeout == 0 {
                    return Ok(Response::ReceiveMessages { page });
                }
                if tokio::time::Instant::now() >= deadline {
                    page.timed_out = true;
                    return Ok(Response::ReceiveMessages { page });
                }
                // Wait owns no roster, lifecycle or database lock. Timeout affects
                // this receive call only, never a task or delivery claim.
                tokio::time::sleep_until(
                    deadline.min(tokio::time::Instant::now() + Duration::from_millis(25)),
                )
                .await;
            }
        }
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

async fn resolve_exact(state: &AppState, target: &str) -> Result<String, ControlError> {
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
            matches.push(id.clone());
        }
    }
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => Err(ControlError::not_found("No exact recipient exists.")),
        _ => Err(ControlError::coded(
            "ambiguous_target",
            "More than one agent matches; use an unambiguous UUID.",
        )),
    }
}

struct AdmissionInput<'a> {
    sender: &'a str,
    target: &'a str,
    message: &'a str,
    key: Option<&'a str>,
    task: bool,
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
    let recipient = resolve_exact(state, target).await?;
    let generation = state
        .interactions
        .current_provider_input_generation(&recipient)
        .await
        .unwrap_or(0);
    let admitted = state
        .interactions
        .admit_agent_message(store::Admission {
            sender,
            recipient: &recipient,
            message,
            idempotency_key: key,
            task,
            generation,
        })
        .await
        .map_err(control_error)?;
    if let Some(app) = app {
        let _ = app.emit("pair-activity-changed", ());
        if task && !admitted.duplicate {
            spawn_pending_tasks(app, &recipient);
        } else if !task && !admitted.duplicate {
            native::spawn_information(app, &recipient);
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
    // Ready/idle observations also give stored information a push opportunity;
    // this path itself can only inspect an already-existing native owner.
    native::spawn_information(app, recipient);
    let app = app.clone();
    let recipient = recipient.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(error) =
            dispatch_pending_queue(Some(&app), &app.state::<AppState>(), &recipient).await
        {
            manager::log_debug(&format!("[WARDIAN] v2 task dispatch: {error}"));
        }
    });
}

/// After a completed dispatch returns, its lease/execution guard has dropped.
/// A changed queue head is distinct admitted work. An unchanged head means busy
/// or unsupported and stops this opportunity without polling or replay.
async fn dispatch_pending_queue(
    app: Option<&AppHandle>,
    state: &AppState,
    recipient: &str,
) -> Result<(), ControlError> {
    loop {
        let before = store::with_db(|conn| store::next_pending_task_id(conn, recipient))
            .map_err(control_error)?;
        if before.is_none() {
            return Ok(());
        }
        let outcome = dispatch_one(app, state, recipient).await;
        let after = store::with_db(|conn| store::next_pending_task_id(conn, recipient))
            .map_err(control_error)?;
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

async fn dispatch_one(
    app: Option<&AppHandle>,
    state: &AppState,
    recipient: &str,
) -> Result<(), ControlError> {
    let info = delivery_target_info(state, recipient).await?;
    if info.provider == "codex" {
        // A background acquisition owns its entire run and shutdown. Further
        // tasks wait for release instead of joining an owner about to exit.
        if active_conversation_lease_for_delivery(&info) {
            return Ok(());
        }
        if status_uses_headless_delivery(&info.status) {
            return dispatch_background_task(app, state, &info).await;
        }
        return native::dispatch_attached_task(state, &info).await;
    }
    if status_uses_headless_delivery(&info.status) {
        return dispatch_background_task(app, state, &info).await;
    }
    // Never wait behind a long-running lifecycle action. A subsequent idle
    // observation or receive call can claim still-pending work.
    let Some(_lifecycle) = state.try_lock_agent_lifecycle(recipient).await else {
        return Ok(());
    };
    let info = delivery_target_info(state, recipient).await?;
    if info.status != "idle"
        || provider_input_blocks_mailbox_drain(state, recipient).await
        || active_conversation_lease_for_delivery(&info)
    {
        return Ok(());
    }
    if state
        .terminal_sessions
        .broker_state(recipient)
        .await
        .is_err()
    {
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
        return Ok(());
    };
    let prompt = message_with_structured_reply_instruction(
        &prepare_claim_context(state, &claim).await?,
        &claim.record.id,
    );
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
            runtime_state: "agent_messaging_task",
            mark_prompt_started: true,
            require_provider_turn_receipt: true,
            payload_sent_detail: None,
            delivery_message_id: Some(claim.record.id.clone()),
        },
    )
    .await;
    let outcome = match &result {
        Ok(_) => "provider_visible",
        Err(error) if error.retry_safe => "failed_before_submit",
        Err(_) => "uncertain",
    };
    state
        .interactions
        .finish_agent_task(&claim, outcome)
        .await
        .map_err(control_error)?;
    Ok(())
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
    let prompt = message_with_structured_reply_instruction(
        &prepare_claim_context(state, &claim).await?,
        &claim.record.id,
    );
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
        },
    )
    .await;
    let outcome = if result.is_ok() {
        "provider_visible"
    } else {
        "uncertain"
    };
    state
        .interactions
        .finish_agent_task(&claim, outcome)
        .await
        .map_err(control_error)?;
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

/// Legacy `wardian reply` to a v2 request uses the same atomic completion path.
pub(super) async fn legacy_reply(
    state: &AppState,
    request_id: &str,
    status: ReplyStatus,
    body: &str,
    origin: Option<&MessageOrigin>,
    app: Option<&AppHandle>,
) -> Result<Option<StructuredReply>, ControlError> {
    if !store::with_db(|conn| store::is_task(conn, request_id)).map_err(control_error)? {
        return Ok(None);
    }
    let Some(MessageOrigin::WardianAgent { session_id }) = origin else {
        return Err(ControlError::coded(
            "unauthorized",
            "A managed reply origin is required.",
        ));
    };
    authenticate(state, session_id).await?;
    let result = state
        .interactions
        .reply_agent_message(session_id, request_id, status, body)
        .await
        .map_err(control_error)?;
    if let Some(app) = app {
        native::spawn_information(app, &result.record.target_session_ids[0]);
    }
    Ok(Some(result.reply))
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

pub(super) fn message_with_structured_reply_instruction(message: &str, request_id: &str) -> String {
    format!(
        "{message}\n\nWardian request id: {request_id}\nWhen finished, execute this command from your shell/tool with the reply body on stdin:\nwardian reply {request_id} --status done --stdin\nUse --status blocked or --status failed if you cannot complete it. Do not print the command as your final answer; run it so Wardian can record the structured reply."
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
