//! Delivery routing with stable remote-client intent and authority fences.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn deliver_message_to_target_with_delivery_options(
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
    stable_interaction_id: Option<&str>,
    client_send_authority: Option<crate::remote::client_messages::ClientSendAuthority>,
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
    let mut retry_safe = true;
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
        } else if let Some(interaction_id) = stable_interaction_id {
            state
                .interactions
                .create_message_durable_with_id(
                    interaction_id.to_string(),
                    sender_session_id,
                    vec![info.uuid.clone()],
                    InteractionBodyRef::Inline {
                        body: outbound_message.clone(),
                    },
                )
                .await
                .map_err(ControlError::request_failed)?
                .id
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
                let request = crate::delivery::LiveSurfacePromptRequest {
                    session_id: target_uuid.clone(),
                    prompt: outbound_message,
                    interaction_id: Some(interaction_id.clone()),
                    input_mode,
                    queue_policy,
                    approval_action: approval_action.cloned(),
                    origin: origin.cloned(),
                    write_fence: None,
                    runtime_state: "live_pty_available",
                    mark_prompt_started: true,
                    require_provider_turn_receipt: true,
                    payload_sent_detail: None,
                    delivery_message_id: None,
                };
                let result = if let Some(authority) = client_send_authority.clone() {
                    crate::delivery::live_surface::submit_live_surface_prompt_with_authority(
                        app, state, request, authority,
                    )
                    .await
                } else {
                    crate::delivery::submit_live_surface_prompt(app, state, request).await
                };
                match result {
                    Ok(result) => {
                        delivered += 1;
                        delivery.push(result.detail);
                    }
                    Err(error) => {
                        retry_safe &= error.retry_safe;
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
                        client_send_authority.clone(),
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
                        Err(failure) => {
                            if client_send_authority.is_some() {
                                let failure_code =
                                    crate::remote::client_messages::authority_failure_code_in_message(
                                        &failure.message,
                                    );
                                let failure = crate::delivery::native_broker::NativeBrokerError {
                                    code: if failure_code == Some("deadline_expired") {
                                        wardian_core::native_transport::NativeDeliveryErrorCode::DeadlineExpired
                                    } else {
                                        wardian_core::native_transport::NativeDeliveryErrorCode::TransportUnavailable
                                    },
                                    message: failure_code
                                        .unwrap_or(
                                            "Native transport failed before provider submission",
                                        )
                                        .to_string(),
                                    provider_boundary_crossed: false,
                                };
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
                        client_send_authority: client_send_authority.clone(),
                    },
                )
                .await
                {
                    HeadlessMessageDelivery::Completed(mut detail) => {
                        if client_send_authority.is_some()
                            && detail.delivery_state != "provider_applied"
                            && !detail.error.as_ref().is_some_and(|error| {
                                matches!(
                                    error.code.as_str(),
                                    "device_revoked"
                                        | "origin_forbidden"
                                        | "grant_required"
                                        | "service_unavailable"
                                        | "deadline_expired"
                                )
                            })
                        {
                            detail.delivery_state = "submitted_unconfirmed".to_string();
                            detail.delivery_phase = Some("submitted_unconfirmed".to_string());
                        }
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
        .with_details(delivery_error_details(&delivery, retry_safe)));
    }
    if !failures.is_empty() {
        return Err(ControlError::request_failed(format!(
            "message delivery failed for {} of {} matched agents: {}",
            failures.len(),
            session_ids.len(),
            failures.join("; ")
        ))
        .with_details(delivery_error_details(&delivery, retry_safe)));
    }
    Ok(delivery)
}

/// Preserve the typed live-surface submission evidence across the control envelope.
fn delivery_error_details(delivery: &[DeliveryDetail], retry_safe: bool) -> serde_json::Value {
    let mut details = delivery_details_json(delivery);
    details["retry_safe"] = serde_json::json!(retry_safe);
    details
}
