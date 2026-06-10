use tauri::AppHandle;
use wardian_core::control::{
    ApprovalAction, DeliveryDetail, DeliveryErrorDetail, DeliveryTransportKind, InteractionBodyRef,
    MessageInputMode, MessageOrigin, QueuePolicy,
};

use crate::state::AppState;
use crate::utils::delivery_transaction::TerminalDeliveryError;

type LiveSurfaceTargetResult = Result<
    (String, String, tokio::sync::mpsc::Sender<Vec<u8>>),
    (Option<LiveSurfaceTarget>, FailedLiveSurfaceAttempt),
>;

#[derive(Debug, Clone)]
pub struct LiveSurfacePromptRequest {
    pub session_id: String,
    pub prompt: String,
    pub interaction_id: Option<String>,
    pub input_mode: MessageInputMode,
    pub queue_policy: QueuePolicy,
    pub approval_action: Option<ApprovalAction>,
    pub origin: Option<MessageOrigin>,
    pub runtime_state: &'static str,
    pub mark_prompt_started: bool,
    pub payload_sent_detail: Option<DeliveryDetail>,
    pub delivery_message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LiveSurfacePromptResult {
    pub interaction_id: String,
    pub detail: wardian_core::control::DeliveryDetail,
}

#[derive(Debug, Clone)]
pub struct LiveSurfaceDeliveryError {
    pub message: String,
    pub detail: Option<DeliveryDetail>,
    pub retry_safe: bool,
}

impl std::fmt::Display for LiveSurfaceDeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LiveSurfaceDeliveryError {}

impl LiveSurfacePromptRequest {
    pub fn message(session_id: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            prompt: prompt.into(),
            interaction_id: None,
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::LiveOnly,
            approval_action: None,
            origin: None,
            runtime_state: "live_pty_available",
            mark_prompt_started: true,
            payload_sent_detail: None,
            delivery_message_id: None,
        }
    }
}

pub async fn submit_live_surface_prompt(
    app: Option<&AppHandle>,
    state: &AppState,
    request: LiveSurfacePromptRequest,
) -> Result<LiveSurfacePromptResult, LiveSurfaceDeliveryError> {
    let delivery_lock = state.delivery_lock_for(&request.session_id).await;
    let _delivery_guard = delivery_lock.lock().await;

    let interaction_id = match request.interaction_id.clone() {
        Some(id) => id,
        None => {
            let sender_session_id = request.origin.as_ref().map(|origin| match origin {
                MessageOrigin::WardianAgent { session_id } => session_id.clone(),
            });
            state
                .interactions
                .create_message_durable(
                    sender_session_id,
                    vec![request.session_id.clone()],
                    redacted_live_prompt_body_ref(&request.prompt),
                )
                .await
                .map_err(|message| LiveSurfaceDeliveryError {
                    message,
                    detail: None,
                    retry_safe: true,
                })?
                .id
        }
    };

    let target_result: LiveSurfaceTargetResult = {
        let agents = state.agents.lock().await;
        if let Some(agent) = agents.get(&request.session_id) {
            match agent.config.lock() {
                Ok(config) => {
                    let config = config.clone();
                    match state.input_senders.try_read() {
                        Ok(senders) => match senders.get(&request.session_id).cloned() {
                            Some(tx) => Ok((config.session_name, config.provider, tx)),
                            None => Err((
                                Some(LiveSurfaceTarget {
                                    name: config.session_name,
                                    provider: config.provider,
                                }),
                                FailedLiveSurfaceAttempt {
                                    runtime_state: missing_sender_runtime_state(
                                        request.runtime_state,
                                    ),
                                    error_code: "no_input_channel",
                                    message: "no input channel".to_string(),
                                    delivery_phase: Some("input_channel_missing".to_string()),
                                    retry_safe: true,
                                },
                            )),
                        },
                        Err(_) => Err((
                            Some(LiveSurfaceTarget {
                                name: config.session_name,
                                provider: config.provider,
                            }),
                            FailedLiveSurfaceAttempt {
                                runtime_state: request.runtime_state,
                                error_code: "input_channel_locked",
                                message: "Input channel temporarily locked".to_string(),
                                delivery_phase: Some("input_channel_locked".to_string()),
                                retry_safe: true,
                            },
                        )),
                    }
                }
                Err(_) => Err((
                    Some(LiveSurfaceTarget {
                        name: request.session_id.clone(),
                        provider: "unknown".to_string(),
                    }),
                    FailedLiveSurfaceAttempt {
                        runtime_state: request.runtime_state,
                        error_code: "config_lock_poisoned",
                        message: format!("Agent {} config lock poisoned", request.session_id),
                        delivery_phase: Some("target_config_failed".to_string()),
                        retry_safe: true,
                    },
                )),
            }
        } else {
            Err((
                None,
                FailedLiveSurfaceAttempt {
                    runtime_state: "target_off",
                    error_code: "agent_not_found",
                    message: format!("Agent {} not found or is off", request.session_id),
                    delivery_phase: Some("target_lookup_failed".to_string()),
                    retry_safe: true,
                },
            ))
        }
    };
    let (name, provider, tx) = match target_result {
        Ok(target) => target,
        Err((target, failure)) => {
            return Err(record_failed_live_surface_attempt(
                state,
                &request,
                &interaction_id,
                target,
                failure,
            )
            .await);
        }
    };

    let outcome = if let (MessageInputMode::ApprovalAction, Some(action)) =
        (request.input_mode, request.approval_action.as_ref())
    {
        match crate::control::submit_approval_action_for_delivery_service(&tx, &provider, action)
            .await
        {
            Ok(outcome) => outcome,
            Err(message) => {
                return Err(record_failed_live_surface_attempt(
                    state,
                    &request,
                    &interaction_id,
                    Some(LiveSurfaceTarget {
                        name: name.clone(),
                        provider: provider.clone(),
                    }),
                    FailedLiveSurfaceAttempt {
                        runtime_state: request.runtime_state,
                        error_code: "send_failed",
                        message,
                        delivery_phase: Some("approval_send_failed".to_string()),
                        retry_safe: true,
                    },
                )
                .await);
            }
        }
    } else {
        if let Err(message) =
            crate::control::wait_for_terminal_ready_for_delivery_service(state, &request.session_id)
                .await
        {
            return Err(record_failed_live_surface_attempt(
                state,
                &request,
                &interaction_id,
                Some(LiveSurfaceTarget {
                    name: name.clone(),
                    provider: provider.clone(),
                }),
                FailedLiveSurfaceAttempt {
                    runtime_state: request.runtime_state,
                    error_code: "not_input_ready",
                    message,
                    delivery_phase: Some("terminal_ready_wait_failed".to_string()),
                    retry_safe: true,
                },
            )
            .await);
        }
        let payload_cursor =
            crate::control::codex_payload_echo_cursor(state, &provider, &request.session_id).await;
        let wait_session_id = request.session_id.clone();
        let wait_provider = provider.clone();
        let wait_prompt = request.prompt.clone();
        let payload_sent_detail = request.payload_sent_detail.clone();
        match crate::utils::terminal_input::submit_prompt_with_outcome_via_sender_after_payload(
            &tx,
            &request.prompt,
            &provider,
            || async move {
                if let Some(detail) = payload_sent_detail.as_ref() {
                    crate::control::push_delivery_for_delivery_service(
                        state,
                        &wait_session_id,
                        detail,
                    )
                    .await;
                }
                crate::control::wait_for_codex_payload_echo_before_submit(
                    state,
                    &wait_provider,
                    &wait_session_id,
                    payload_cursor.as_deref(),
                    &wait_prompt,
                )
                .await;
            },
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(record_terminal_delivery_error(
                    state,
                    &request,
                    &interaction_id,
                    &name,
                    &provider,
                    error,
                )
                .await);
            }
        }
    };

    let detail = wardian_core::control::DeliveryDetail {
        uuid: request.session_id.clone(),
        name,
        provider: provider.clone(),
        runtime_state: request.runtime_state.to_string(),
        delivery_state: outcome.delivery_state,
        input_mode: request.input_mode,
        queue_policy: request.queue_policy,
        message_id: Some(
            request
                .delivery_message_id
                .clone()
                .unwrap_or_else(|| interaction_id.clone()),
        ),
        delivery_phase: Some(outcome.delivery_phase),
        observed_state: outcome.observed_state,
        reason: outcome.reason,
        profile: Some(crate::utils::delivery_profile::delivery_profile(&provider).provider),
        error: None,
    };

    let generation = state
        .interactions
        .current_provider_input_generation(&request.session_id)
        .await
        .unwrap_or(0);
    state
        .interactions
        .record_delivery_attempt_durable(
            &interaction_id,
            &request.session_id,
            DeliveryTransportKind::LiveSurface,
            generation,
            &detail.runtime_state,
            &detail.delivery_state,
            detail.delivery_phase.clone(),
            detail.observed_state.clone(),
            detail.reason.clone(),
            detail.error.clone(),
        )
        .await
        .map_err(|message| LiveSurfaceDeliveryError {
            message,
            detail: Some(detail.clone()),
            retry_safe: false,
        })?;
    crate::control::push_delivery_for_delivery_service(state, &request.session_id, &detail).await;

    if request.mark_prompt_started {
        crate::control::mark_delivered_agents_prompt_started_for_delivery_service(
            app,
            state,
            std::slice::from_ref(&request.session_id),
        )
        .await;
    }

    Ok(LiveSurfacePromptResult {
        interaction_id,
        detail,
    })
}

#[derive(Debug, Clone)]
struct LiveSurfaceTarget {
    name: String,
    provider: String,
}

#[derive(Debug, Clone)]
struct FailedLiveSurfaceAttempt {
    runtime_state: &'static str,
    error_code: &'static str,
    message: String,
    delivery_phase: Option<String>,
    retry_safe: bool,
}

fn missing_sender_runtime_state(request_runtime_state: &'static str) -> &'static str {
    if request_runtime_state == "live_pty_available" {
        "restored_without_sender"
    } else {
        request_runtime_state
    }
}

async fn record_terminal_delivery_error(
    state: &AppState,
    request: &LiveSurfacePromptRequest,
    interaction_id: &str,
    name: &str,
    provider: &str,
    error: TerminalDeliveryError,
) -> LiveSurfaceDeliveryError {
    record_failed_live_surface_attempt(
        state,
        request,
        interaction_id,
        Some(LiveSurfaceTarget {
            name: name.to_string(),
            provider: provider.to_string(),
        }),
        FailedLiveSurfaceAttempt {
            runtime_state: request.runtime_state,
            error_code: "send_failed",
            message: error.message,
            delivery_phase: Some(error.phase.to_string()),
            retry_safe: error.retry_safe,
        },
    )
    .await
}

async fn record_failed_live_surface_attempt(
    state: &AppState,
    request: &LiveSurfacePromptRequest,
    interaction_id: &str,
    target: Option<LiveSurfaceTarget>,
    failure: FailedLiveSurfaceAttempt,
) -> LiveSurfaceDeliveryError {
    let target = target.unwrap_or_else(|| LiveSurfaceTarget {
        name: request.session_id.clone(),
        provider: "unknown".to_string(),
    });
    let mut detail = DeliveryDetail {
        uuid: request.session_id.clone(),
        name: target.name,
        provider: target.provider.clone(),
        runtime_state: failure.runtime_state.to_string(),
        delivery_state: "failed".to_string(),
        input_mode: request.input_mode,
        queue_policy: request.queue_policy,
        message_id: Some(
            request
                .delivery_message_id
                .clone()
                .unwrap_or_else(|| interaction_id.to_string()),
        ),
        delivery_phase: failure.delivery_phase,
        observed_state: None,
        reason: None,
        profile: Some(crate::utils::delivery_profile::delivery_profile(&target.provider).provider),
        error: Some(DeliveryErrorDetail {
            code: failure.error_code.to_string(),
            message: failure.message.clone(),
        }),
    };
    if failure.retry_safe {
        detail.reason = Some("delivery did not reach the provider input".to_string());
    } else {
        detail.reason =
            Some("terminal state is partial or unknown after payload delivery".to_string());
    }

    let generation = state
        .interactions
        .current_provider_input_generation(&request.session_id)
        .await
        .unwrap_or(0);
    let persist_result = state
        .interactions
        .record_delivery_attempt_durable(
            interaction_id,
            &request.session_id,
            DeliveryTransportKind::LiveSurface,
            generation,
            &detail.runtime_state,
            &detail.delivery_state,
            detail.delivery_phase.clone(),
            detail.observed_state.clone(),
            detail.reason.clone(),
            detail.error.clone(),
        )
        .await;
    crate::control::push_delivery_for_delivery_service(state, &request.session_id, &detail).await;

    let message = match persist_result {
        Ok(_) => failure.message,
        Err(persist_error) => format!("{}; {persist_error}", failure.message),
    };
    LiveSurfaceDeliveryError {
        message,
        detail: Some(detail),
        retry_safe: failure.retry_safe,
    }
}

fn redacted_live_prompt_body_ref(prompt: &str) -> InteractionBodyRef {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(prompt.as_bytes());
    InteractionBodyRef::Inline {
        body: format!(
            "[redacted live prompt; sha256={:x}; bytes={}]",
            digest,
            prompt.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_defaults_use_message_mode_and_live_only_policy() {
        let request = LiveSurfacePromptRequest::message("agent-1", "hello");

        assert_eq!(request.session_id, "agent-1");
        assert_eq!(request.prompt, "hello");
        assert_eq!(request.input_mode, MessageInputMode::Message);
        assert_eq!(request.queue_policy, QueuePolicy::LiveOnly);
        assert_eq!(request.runtime_state, "live_pty_available");
        assert!(request.mark_prompt_started);
    }
}
