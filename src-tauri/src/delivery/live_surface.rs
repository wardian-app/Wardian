use tauri::AppHandle;
use wardian_core::control::{
    ApprovalAction, DeliveryDetail, DeliveryErrorDetail, DeliveryTransportKind, InteractionBodyRef,
    MessageInputMode, MessageOrigin, QueuePolicy,
};

use crate::state::AppState;
use crate::utils::delivery_transaction::{BrokerTerminalInputSink, TerminalDeliveryError};

type LiveSurfaceTargetResult =
    Result<(String, String), (Option<LiveSurfaceTarget>, FailedLiveSurfaceAttempt)>;

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

fn automatic_payload_started_detail(
    request: &LiveSurfacePromptRequest,
    interaction_id: &str,
    name: &str,
    provider: &str,
) -> Option<DeliveryDetail> {
    matches!(
        request.input_mode,
        MessageInputMode::Message | MessageInputMode::Command
    )
    .then(|| DeliveryDetail {
        uuid: request.session_id.clone(),
        name: name.to_string(),
        provider: provider.to_string(),
        runtime_state: request.runtime_state.to_string(),
        delivery_state: "submit_started".to_string(),
        input_mode: request.input_mode,
        queue_policy: request.queue_policy,
        message_id: Some(
            request
                .delivery_message_id
                .clone()
                .unwrap_or_else(|| interaction_id.to_string()),
        ),
        delivery_phase: Some("payload_sent".to_string()),
        observed_state: Some("payload_sent".to_string()),
        reason: None,
        profile: Some(crate::utils::delivery_profile::delivery_profile(provider).provider),
        error: None,
    })
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
                Ok(config) => Ok((config.session_name.clone(), config.provider.clone())),
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
    let (name, provider) = match target_result {
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
    if state
        .terminal_sessions
        .broker_state(&request.session_id)
        .await
        .is_err()
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
                runtime_state: missing_sender_runtime_state(request.runtime_state),
                error_code: "no_input_channel",
                message: "no input channel".to_string(),
                delivery_phase: Some("input_channel_missing".to_string()),
                retry_safe: true,
            },
        )
        .await);
    }
    let native_write_receipts = match state
        .terminal_sessions
        .native_write_receipts_enabled(&request.session_id)
        .await
    {
        Ok(enabled) => enabled,
        Err(error) => {
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
                    error_code: "input_receipt_unavailable",
                    message: error.to_string(),
                    delivery_phase: Some("input_receipt_check_failed".to_string()),
                    retry_safe: true,
                },
            )
            .await);
        }
    };
    let input =
        BrokerTerminalInputSink::new(state.terminal_sessions.clone(), request.session_id.clone());
    // This event is emitted after the payload has been acknowledged by the
    // native PTY writer but before the submit key. It gives send-and-watch an
    // ordering boundary that precedes every provider response for this exact
    // message.
    let payload_sent_detail = request
        .payload_sent_detail
        .clone()
        .or_else(|| automatic_payload_started_detail(&request, &interaction_id, &name, &provider));
    let requires_provider_turn_receipt = native_write_receipts
        && matches!(
            request.input_mode,
            MessageInputMode::Message | MessageInputMode::Command
        );
    let mut turn_start_cursor = None;
    let outcome = if let (MessageInputMode::ApprovalAction, Some(action)) =
        (request.input_mode, request.approval_action.as_ref())
    {
        match crate::control::submit_approval_action_for_delivery_service(&input, &provider, action)
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
                        message: message.message,
                        delivery_phase: Some(message.phase.to_string()),
                        retry_safe: message.retry_safe,
                    },
                )
                .await);
            }
        }
    } else if provider == "prime" {
        // Prime's daemon is the agent and the TUI is only one of its clients,
        // so Wardian hands the message to the supervisor instead of typing it.
        // That answers with an acknowledgement, which is a fact about the
        // message rather than an inference from what the screen paints next --
        // so there is no readiness wait and no turn-start cursor here.
        let bound_provider_session = {
            let agents = state.agents.lock().await;
            agents
                .get(&request.session_id)
                .and_then(|agent| agent.config.lock().ok())
                .and_then(|config| config.resume_session.clone())
        };
        let selector = crate::delivery::prime_send::wait_for_worker_selector(
            &request.session_id,
            bound_provider_session.as_deref(),
        )
        .await
        .unwrap_or_default();

        match crate::delivery::prime_send::deliver(crate::delivery::prime_send::PrimeSendRequest {
            selector: &selector,
            prompt: &request.prompt,
        })
        .await
        {
            Ok(outcome) => {
                if let Some(detail) = payload_sent_detail.clone() {
                    persist_live_surface_delivery_detail(
                        state,
                        &interaction_id,
                        &request.session_id,
                        &detail,
                    )
                    .await
                    .map_err(|message| LiveSurfaceDeliveryError {
                        message,
                        detail: Some(detail.clone()),
                        retry_safe: true,
                    })?;
                    crate::control::push_delivery_for_delivery_service(
                        state,
                        &request.session_id,
                        &detail,
                    )
                    .await;
                }
                outcome
            }
            Err(error) => {
                crate::manager::log_debug(&format!(
                    "[Wardian] Prime send for {} failed at {}: {}",
                    request.session_id, error.phase, error.message
                ));
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
        turn_start_cursor = if requires_provider_turn_receipt {
            match crate::control::provider_turn_start_cursor(state, &request.session_id).await {
                Ok(cursor) => Some(cursor),
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
                            error_code: "turn_start_watch_unavailable",
                            message,
                            delivery_phase: Some("turn_start_cursor_failed".to_string()),
                            retry_safe: true,
                        },
                    )
                    .await);
                }
            }
        } else {
            None
        };
        let wait_session_id = request.session_id.clone();
        let payload_session_id = request.session_id.clone();
        let payload_interaction_id = interaction_id.clone();
        let payload_sent_detail = payload_sent_detail.clone();
        match crate::utils::terminal_input::submit_prompt_with_outcome_via_sender_after_payload(
            &input,
            &request.prompt,
            &provider,
            move || async move {
                if let Some(detail) = payload_sent_detail {
                    persist_live_surface_delivery_detail(
                        state,
                        &payload_interaction_id,
                        &payload_session_id,
                        &detail,
                    )
                    .await
                    .map_err(|message| {
                        TerminalDeliveryError::terminal_state_unknown(
                            "payload_receipt_persist_failed",
                            message,
                        )
                    })?;
                    crate::control::push_delivery_for_delivery_service(
                        state,
                        &wait_session_id,
                        &detail,
                    )
                    .await;
                }
                Ok(())
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

    let mut detail = wardian_core::control::DeliveryDetail {
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

    persist_live_surface_delivery_detail(state, &interaction_id, &request.session_id, &detail)
        .await
        .map_err(|message| LiveSurfaceDeliveryError {
            message,
            detail: Some(detail.clone()),
            retry_safe: false,
        })?;
    crate::control::push_delivery_for_delivery_service(state, &request.session_id, &detail).await;

    if let Some(turn_start_cursor) = turn_start_cursor {
        if let Err(message) = crate::control::wait_for_provider_turn_started_after_submit(
            state,
            &request.session_id,
            &turn_start_cursor,
        )
        .await
        {
            return Err(record_failed_live_surface_attempt(
                state,
                &request,
                &interaction_id,
                Some(LiveSurfaceTarget {
                    name: detail.name.clone(),
                    provider: provider.clone(),
                }),
                FailedLiveSurfaceAttempt {
                    runtime_state: request.runtime_state,
                    error_code: "provider_turn_start_timeout",
                    message,
                    delivery_phase: Some("provider_turn_start_timeout".to_string()),
                    retry_safe: false,
                },
            )
            .await);
        }

        detail.delivery_state = "provider_accepted".to_string();
        detail.delivery_phase = Some("turn_started".to_string());
        detail.observed_state = Some("turn_started".to_string());
        detail.reason = Some(
            "provider emitted a turn-start event after native terminal submission".to_string(),
        );
        persist_live_surface_delivery_detail(state, &interaction_id, &request.session_id, &detail)
            .await
            .map_err(|message| LiveSurfaceDeliveryError {
                message,
                detail: Some(detail.clone()),
                retry_safe: false,
            })?;
        crate::control::push_delivery_for_delivery_service(state, &request.session_id, &detail)
            .await;
    } else if request.mark_prompt_started {
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

async fn persist_live_surface_delivery_detail(
    state: &AppState,
    interaction_id: &str,
    session_id: &str,
    detail: &DeliveryDetail,
) -> Result<(), String> {
    let generation = state
        .interactions
        .current_provider_input_generation(session_id)
        .await
        .unwrap_or(0);
    state
        .interactions
        .record_delivery_attempt_durable(
            interaction_id,
            session_id,
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
        .map(|_| ())
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

    #[test]
    fn live_message_gets_a_submit_started_event_before_the_submit_key() {
        let mut request = LiveSurfacePromptRequest::message("agent-1", "hello");
        request.delivery_message_id = Some("msg_1".to_string());

        let detail = automatic_payload_started_detail(&request, "int_1", "Coder", "codex")
            .expect("message delivery detail");

        assert_eq!(detail.delivery_state, "submit_started");
        assert_eq!(detail.delivery_phase.as_deref(), Some("payload_sent"));
        assert_eq!(detail.message_id.as_deref(), Some("msg_1"));
    }

    #[test]
    fn live_command_gets_the_same_payload_receipt_boundary() {
        let mut request = LiveSurfacePromptRequest::message("agent-1", "/status");
        request.input_mode = MessageInputMode::Command;

        let detail = automatic_payload_started_detail(&request, "int_1", "Coder", "codex")
            .expect("command delivery detail");

        assert_eq!(detail.delivery_state, "submit_started");
        assert_eq!(detail.input_mode, MessageInputMode::Command);
    }

    #[test]
    fn approval_delivery_does_not_emit_a_message_submit_started_event() {
        let mut request = LiveSurfacePromptRequest::message("agent-1", "hello");
        request.input_mode = MessageInputMode::ApprovalAction;

        assert!(automatic_payload_started_detail(&request, "int_1", "Coder", "codex").is_none());
    }
}
