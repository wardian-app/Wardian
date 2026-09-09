//! Leased background message execution and native Codex dispatch.
use super::*;

pub(super) async fn deliver_headless_message(
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
        lifecycle_guard,
        orchestration,
        parent_interaction_id,
    } = request;
    // Direct offline delivery runs a provider against the target agent's
    // workspace. Hold the same home-wide shared guard as automation drives
    // before taking a conversation lease, so a managed-worktree deletion
    // cannot remove that workspace before or during provider execution.
    let _headless_execution =
        match wardian_core::automation_execution_lock::acquire_headless_execution_guard() {
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
    // lifecycle gate. Automations and lifecycle mutations use the same order, so
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
    let lifecycle_guard = match lifecycle_guard {
        Some(guard) => guard,
        None => match state.try_lock_agent_lifecycle(&info.uuid).await {
            Some(guard) => guard,
            None => {
                return HeadlessMessageDelivery::Busy(Box::new(
                    delivery_target_info(state, &info.uuid)
                        .await
                        .unwrap_or_else(|_| info.clone()),
                ));
            }
        },
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
    let codex_generation = if current_info.provider == "codex" {
        Some(
            state
                .interactions
                .start_provider_input_generation(
                    &current_info.uuid,
                    ProviderInputReadiness::Booting,
                    None,
                )
                .await
                .generation,
        )
    } else {
        None
    };
    drop(lifecycle_guard);

    if let Some(generation) = codex_generation {
        return codex_background::deliver(
            state,
            HeadlessMessageDeliveryRequest {
                app,
                info: &current_info,
                interaction_id,
                prompt,
                input_mode,
                queue_policy,
                origin,
                timeout,
                lifecycle_guard: None,
                orchestration,
                parent_interaction_id,
            },
            lease_guard,
            generation,
        )
        .await;
    }

    let result = crate::delivery::run_headless_process_prompt(
        state,
        crate::delivery::HeadlessProcessPromptRequest {
            node: "message_delivery".to_string(),
            provider: current_info.provider.clone(),
            cwd: current_info.cwd.clone(),
            prompt: prompt.to_string(),
            session_id: current_info.uuid.clone(),
            memory_agent_id: Some(current_info.uuid.clone()),
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
