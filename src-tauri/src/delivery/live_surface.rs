use std::path::{Path, PathBuf};
use tauri::AppHandle;
use wardian_core::control::{
    ApprovalAction, DeliveryDetail, DeliveryErrorDetail, DeliveryTransportKind, InteractionBodyRef,
    MessageInputMode, MessageOrigin, QueuePolicy,
};

use crate::state::AppState;
use crate::utils::delivery_transaction::{BrokerTerminalInputSink, TerminalDeliveryError};

type LiveSurfaceTargetResult = Result<
    (String, String, wardian_core::models::AgentConfig),
    (Option<LiveSurfaceTarget>, FailedLiveSurfaceAttempt),
>;

#[derive(Debug, Clone)]
struct OpenCodeReceiptBaseline {
    db_path: PathBuf,
    baseline_part_rowid: i64,
    provider_session_id: Option<String>,
    workspace: PathBuf,
    wardian_session_id: String,
    created_after_ms: i64,
    normalized_prompt: String,
    runtime_generation: u64,
}

async fn capture_opencode_receipt_baseline(
    state: &AppState,
    session_id: &str,
    prompt: &str,
    config: &wardian_core::models::AgentConfig,
) -> Result<OpenCodeReceiptBaseline, String> {
    let broker_state = state
        .terminal_sessions
        .broker_state(session_id)
        .await
        .map_err(|error| format!("OpenCode runtime identity unavailable: {error}"))?;
    let provider_session_id = crate::manager::opencode::opencode_telemetry_session_id(config);
    if config.resume_session.is_some() && provider_session_id.is_none() {
        return Err("OpenCode resumed session has no valid provider session identity".to_string());
    }
    let workspace = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id);
    let created_after_ms = chrono::Utc::now().timestamp_millis();
    let database = tokio::task::spawn_blocking(opencode_database_baseline)
        .await
        .map_err(|error| format!("OpenCode receipt baseline task failed: {error}"))??;

    Ok(OpenCodeReceiptBaseline {
        db_path: database.0,
        baseline_part_rowid: database.1,
        provider_session_id,
        workspace,
        wardian_session_id: session_id.to_string(),
        created_after_ms,
        normalized_prompt: crate::utils::terminal_input::normalize_prompt_for_terminal_submit(
            prompt,
        ),
        runtime_generation: broker_state.runtime_generation,
    })
}

fn opencode_database_baseline() -> Result<(PathBuf, i64), String> {
    let db_path = crate::manager::opencode::opencode_database_path()
        .ok_or_else(|| "OpenCode database is unavailable".to_string())?;
    let baseline = opencode_database_baseline_from_path(&db_path)?;
    Ok((db_path, baseline))
}

fn opencode_database_baseline_from_path(db_path: &Path) -> Result<i64, String> {
    let connection =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
    connection
        .prepare(
            "SELECT p.rowid
             FROM part p
             JOIN message m ON m.id = p.message_id
             WHERE p.session_id = m.session_id
             LIMIT 0",
        )
        .map_err(|error| error.to_string())?;
    let baseline = connection
        .query_row("SELECT COALESCE(MAX(rowid), 0) FROM part", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())?;
    Ok(baseline)
}

async fn wait_for_opencode_receipt(baseline: &OpenCodeReceiptBaseline) -> Result<String, String> {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
    const RECEIPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    let started = std::time::Instant::now();
    while started.elapsed() < RECEIPT_TIMEOUT {
        let poll = baseline.clone();
        let result = tokio::task::spawn_blocking(move || poll_opencode_receipt(&poll))
            .await
            .map_err(|error| format!("OpenCode receipt poll failed: {error}"))??;
        if let Some(session_id) = result {
            return Ok(session_id);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err("Timed out waiting for OpenCode to persist the submitted user request".to_string())
}

fn poll_opencode_receipt(baseline: &OpenCodeReceiptBaseline) -> Result<Option<String>, String> {
    let session_id = baseline.provider_session_id.clone().or_else(|| {
        crate::manager::opencode::opencode_recent_session_for_workspace(
            &baseline.workspace,
            baseline.created_after_ms,
            &baseline.wardian_session_id,
        )
    });
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    opencode_database_contains_submitted_user_part(
        &baseline.db_path,
        baseline.baseline_part_rowid,
        &session_id,
        &baseline.normalized_prompt,
    )
    .map(|accepted| accepted.then_some(session_id))
}

fn opencode_database_contains_submitted_user_part(
    db_path: &Path,
    baseline_part_rowid: i64,
    provider_session_id: &str,
    normalized_prompt: &str,
) -> Result<bool, String> {
    let connection =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT p.data, m.data
             FROM part p
             JOIN message m ON m.id = p.message_id
             WHERE p.rowid > ?1
               AND p.session_id = ?2
               AND m.session_id = ?2
             ORDER BY p.rowid
             LIMIT 128",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            rusqlite::params![baseline_part_rowid, provider_session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|error| error.to_string())?;

    for row in rows {
        let (part_data, message_data) = row.map_err(|error| error.to_string())?;
        let message: serde_json::Value = serde_json::from_str(&message_data)
            .map_err(|error| format!("invalid OpenCode message JSON: {error}"))?;
        if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
            continue;
        }
        let part: serde_json::Value = serde_json::from_str(&part_data)
            .map_err(|error| format!("invalid OpenCode part JSON: {error}"))?;
        if part.get("type").and_then(serde_json::Value::as_str) != Some("text")
            || part
                .get("synthetic")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        {
            continue;
        }
        let metadata = part.get("metadata");
        let metadata_kind = metadata
            .and_then(|value| value.get("kind"))
            .and_then(serde_json::Value::as_str);
        let input_origin = metadata
            .and_then(|value| value.get("input_origin"))
            .and_then(serde_json::Value::as_str);
        if matches!(metadata_kind, Some("editor_context" | "internal"))
            || matches!(
                input_origin,
                Some("context_injection" | "provider_internal")
            )
        {
            continue;
        }
        let Some(text) = part.get("text").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if crate::utils::terminal_input::normalize_prompt_for_terminal_submit(text)
            == normalized_prompt
        {
            return Ok(true);
        }
    }
    Ok(false)
}

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
    /// Automated delivery waits for provider-confirmed turn start so it can
    /// safely decide whether a queued message may be retried. A direct human
    /// terminal submission is complete once the native PTY has flushed it.
    pub require_provider_turn_receipt: bool,
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
            require_provider_turn_receipt: false,
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
) -> Result<LiveSurfacePromptResult, Box<LiveSurfaceDeliveryError>> {
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
                Ok(config) => Ok((
                    config.session_name.clone(),
                    config.provider.clone(),
                    config.clone(),
                )),
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
                        observed_state: None,
                        reason: None,
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
                    observed_state: None,
                    reason: None,
                    retry_safe: true,
                },
            ))
        }
    };
    let (name, provider, config) = match target_result {
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
                observed_state: None,
                reason: None,
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
                    observed_state: None,
                    reason: None,
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
    // OpenCode's SQLite acceptance is the provider-owned write boundary even
    // when the PTY runtime cannot acknowledge individual native writes.
    let requires_provider_turn_receipt = request.require_provider_turn_receipt
        && (native_write_receipts || provider == "opencode")
        && matches!(
            request.input_mode,
            MessageInputMode::Message | MessageInputMode::Command
        );
    let mut turn_start_cursor = None;
    let mut opencode_receipt_baseline = None;
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
                        observed_state: None,
                        reason: None,
                        retry_safe: message.retry_safe,
                    },
                )
                .await);
            }
        }
    } else {
        // Antigravity creates its durable provider-owned user step while it
        // finishes drawing the initial compose prompt. Capture the watch
        // cursor first so that real receipt remains observable after the
        // terminal is ready to accept the queued payload.
        if requires_provider_turn_receipt && provider == "antigravity" {
            turn_start_cursor = match crate::control::provider_turn_start_cursor(
                state,
                &request.session_id,
            )
            .await
            {
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
                            observed_state: None,
                            reason: None,
                            retry_safe: true,
                        },
                    )
                    .await);
                }
            };
        }
        if let Err(message) =
            crate::control::wait_for_terminal_ready_for_delivery_service(state, &request.session_id)
                .await
        {
            let composer_stalled = provider == "codex"
                && crate::delivery::codex_composer::session_has_stalled_composer(
                    state,
                    &request.session_id,
                )
                .await
                .unwrap_or(false);
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
                    error_code: if composer_stalled {
                        "provider_composer_stalled"
                    } else {
                        "not_input_ready"
                    },
                    message,
                    delivery_phase: Some(if composer_stalled {
                        "provider_composer_stalled".to_string()
                    } else {
                        "terminal_ready_wait_failed".to_string()
                    }),
                    observed_state: composer_stalled
                        .then(|| "payload_pending_in_composer".to_string()),
                    reason: composer_stalled.then(|| format!(
                        "Codex has an unsubmitted payload in its composer; retrying cannot succeed until `wardian agent restart {}` clears it while preserving the agent and session history",
                        request.session_id
                    )),
                    retry_safe: !composer_stalled,
                },
            )
            .await);
        }
        opencode_receipt_baseline = if requires_provider_turn_receipt && provider == "opencode" {
            match capture_opencode_receipt_baseline(
                state,
                &request.session_id,
                &request.prompt,
                &config,
            )
            .await
            {
                Ok(baseline) => Some(baseline),
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
                            error_code: "opencode_receipt_unavailable",
                            message,
                            delivery_phase: Some("receipt_baseline_failed".to_string()),
                            observed_state: None,
                            reason: Some(
                                "OpenCode acceptance could not be bounded before input; no payload was submitted"
                                    .to_string(),
                            ),
                            retry_safe: true,
                        },
                    )
                    .await);
                }
            }
        } else {
            None
        };
        if requires_provider_turn_receipt && turn_start_cursor.is_none() {
            turn_start_cursor = match crate::control::provider_turn_start_cursor(
                state,
                &request.session_id,
            )
            .await
            {
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
                            observed_state: None,
                            reason: None,
                            retry_safe: true,
                        },
                    )
                    .await);
                }
            };
        }
        crate::manager::clear_agent_interrupted_for_session(state, &request.session_id).await;
        let wait_session_id = request.session_id.clone();
        let payload_session_id = request.session_id.clone();
        let payload_interaction_id = interaction_id.clone();
        let payload_sent_detail = payload_sent_detail.clone();
        let apply_provider = provider.clone();
        let apply_session_id = request.session_id.clone();
        let apply_prompt =
            crate::utils::terminal_input::normalize_prompt_for_terminal_submit(&request.prompt);
        let apply_cursor = turn_start_cursor.clone();
        let require_payload_apply_evidence = requires_provider_turn_receipt;
        match crate::utils::terminal_input::submit_prompt_with_outcome_via_sender_after_payload_and_before_submit(
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
            move || async move {
                if apply_provider != "codex" || !require_payload_apply_evidence {
                    return Ok(());
                }
                let cursor = apply_cursor.as_deref().ok_or_else(|| {
                    TerminalDeliveryError::terminal_state_unknown(
                        "payload_apply_unconfirmed",
                        format!(
                            "No watch cursor was available to confirm Codex payload application for {apply_session_id}; Return was not sent"
                        ),
                    )
                })?;
                crate::delivery::codex_composer::wait_for_payload_applied_before_submit(
                    state,
                    &apply_session_id,
                    cursor,
                    &apply_prompt,
                )
                .await
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

    if let Some(opencode_receipt_baseline) = opencode_receipt_baseline {
        let accepted_session = match wait_for_opencode_receipt(&opencode_receipt_baseline).await {
            Ok(session_id) => session_id,
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
                        error_code: "opencode_receipt_timeout",
                        message,
                        delivery_phase: Some("provider_turn_start_timeout".to_string()),
                        observed_state: None,
                        reason: Some(
                            "OpenCode input was submitted but no session-bound native acceptance receipt was observed; automatic retry is unsafe"
                                .to_string(),
                        ),
                        retry_safe: false,
                    },
                )
                .await);
            }
        };
        if let Err(message) = crate::manager::record_agent_turn_started_for_watch_at_generation(
            state,
            &request.session_id,
            opencode_receipt_baseline.runtime_generation,
        )
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
                    error_code: "stale_runtime_receipt",
                    message: format!("{message}; provider session {accepted_session}"),
                    delivery_phase: Some("provider_turn_start_stale".to_string()),
                    observed_state: None,
                    reason: Some(
                        "The provider receipt was observed after the Wardian runtime changed; the turn is ambiguous and must be reconciled before retry"
                            .to_string(),
                    ),
                    retry_safe: false,
                },
            )
                .await);
        }
    }

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
            let composer_stalled = provider == "codex"
                && crate::delivery::codex_composer::session_has_stalled_composer(
                    state,
                    &request.session_id,
                )
                .await
                .unwrap_or(false);
            let (error_code, delivery_phase, observed_state, reason) = if composer_stalled {
                (
                    "provider_composer_stalled",
                    "provider_composer_stalled",
                    Some("payload_pending_in_composer".to_string()),
                    Some(format!(
                        "Codex remained idle with the submitted payload in its composer; retrying cannot succeed until `wardian agent restart {}` clears the composer while preserving the agent and session history",
                        request.session_id
                    )),
                )
            } else {
                (
                    "provider_turn_start_timeout",
                    "provider_turn_start_timeout",
                    None,
                    Some(
                        "No provider turn-start was observed and no pending Codex composer payload was detected; reconcile provider history before deciding whether to retry"
                            .to_string(),
                    ),
                )
            };
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
                    error_code,
                    message,
                    delivery_phase: Some(delivery_phase.to_string()),
                    observed_state,
                    reason,
                    retry_safe: false,
                },
            )
            .await);
        }

        detail.delivery_state = "provider_accepted".to_string();
        detail.delivery_phase = Some("turn_started".to_string());
        detail.observed_state = Some("turn_started".to_string());
        detail.reason = Some(if provider == "opencode" {
            "OpenCode persisted a new exact user request in the owned session after native terminal submission".to_string()
        } else {
            "provider emitted a turn-start event after native terminal submission".to_string()
        });
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
    observed_state: Option<String>,
    reason: Option<String>,
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
) -> Box<LiveSurfaceDeliveryError> {
    let default_payload_reason = (error.phase == "payload_apply_unconfirmed").then(|| {
        "The PTY accepted the payload bytes, but Codex did not prove that its composer applied them; Return was withheld and automatic retry is unsafe"
            .to_string()
    });
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
            error_code: if error.phase == "payload_apply_unconfirmed" {
                "payload_apply_unconfirmed"
            } else {
                "send_failed"
            },
            message: error.message,
            delivery_phase: Some(error.phase.to_string()),
            observed_state: error.observed_state,
            reason: error.reason.or(default_payload_reason),
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
) -> Box<LiveSurfaceDeliveryError> {
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
        observed_state: failure.observed_state,
        reason: failure.reason,
        profile: Some(crate::utils::delivery_profile::delivery_profile(&target.provider).provider),
        error: Some(DeliveryErrorDetail {
            code: failure.error_code.to_string(),
            message: failure.message.clone(),
        }),
    };
    if detail.reason.is_some() {
        // A provider-specific diagnosis is more actionable than the generic
        // retry-safety wording below.
    } else if failure.retry_safe {
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
    Box::new(LiveSurfaceDeliveryError {
        message,
        detail: Some(detail),
        retry_safe: failure.retry_safe,
    })
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
        assert!(!request.require_provider_turn_receipt);
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

    fn receipt_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().expect("fixture temp dir");
        let db_path = temp.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&db_path).expect("fixture database");
        connection
            .execute_batch(
                r#"
                CREATE TABLE message (
                    id text PRIMARY KEY,
                    session_id text NOT NULL,
                    time_created integer,
                    time_updated integer,
                    data text NOT NULL
                );
                CREATE TABLE part (
                    id text PRIMARY KEY,
                    message_id text NOT NULL,
                    session_id text NOT NULL,
                    time_created integer,
                    time_updated integer,
                    data text NOT NULL
                );
                "#,
            )
            .expect("fixture schema");
        drop(connection);
        (temp, db_path)
    }

    fn insert_user_part(
        db_path: &Path,
        message_id: &str,
        part_id: &str,
        message_session_id: &str,
        part_session_id: &str,
        text: &str,
        part_data_suffix: &str,
    ) {
        let connection = rusqlite::Connection::open(db_path).expect("open fixture database");
        connection
            .execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, 1, 1, ?3)",
                rusqlite::params![message_id, message_session_id, r#"{"role":"user"}"#],
            )
            .expect("insert fixture message");
        let part_data = format!(r#"{{"type":"text","text":{text:?}{part_data_suffix}}}"#);
        connection
            .execute(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, 2, 2, ?4)",
                rusqlite::params![part_id, message_id, part_session_id, part_data],
            )
            .expect("insert fixture part");
    }

    fn fixture_baseline(db_path: &Path) -> i64 {
        rusqlite::Connection::open(db_path)
            .expect("open fixture database")
            .query_row("SELECT COALESCE(MAX(rowid), 0) FROM part", [], |row| {
                row.get(0)
            })
            .expect("read fixture baseline")
    }

    #[test]
    fn opencode_receipt_requires_a_new_exact_user_part_on_the_owned_session() {
        let (_temp, db_path) = receipt_fixture();
        insert_user_part(
            &db_path,
            "old-message",
            "old-part",
            "ses-owned",
            "ses-owned",
            "repeat me",
            "",
        );
        let baseline = fixture_baseline(&db_path);

        assert!(!opencode_database_contains_submitted_user_part(
            &db_path,
            baseline,
            "ses-owned",
            "repeat me"
        )
        .expect("old row query"));

        insert_user_part(
            &db_path,
            "foreign-message",
            "foreign-part",
            "ses-foreign",
            "ses-foreign",
            "repeat me",
            "",
        );
        assert!(!opencode_database_contains_submitted_user_part(
            &db_path,
            baseline,
            "ses-owned",
            "repeat me"
        )
        .expect("foreign row query"));

        insert_user_part(
            &db_path,
            "synthetic-message",
            "synthetic-part",
            "ses-owned",
            "ses-owned",
            "repeat me",
            r#", "synthetic":true, "metadata":{"kind":"editor_context"}"#,
        );
        assert!(!opencode_database_contains_submitted_user_part(
            &db_path,
            baseline,
            "ses-owned",
            "repeat me"
        )
        .expect("synthetic row query"));

        insert_user_part(
            &db_path,
            "accepted-message",
            "accepted-part",
            "ses-owned",
            "ses-owned",
            " repeat me\r\n",
            "",
        );
        assert!(opencode_database_contains_submitted_user_part(
            &db_path,
            baseline,
            "ses-owned",
            "repeat me"
        )
        .expect("accepted row query"));
    }

    #[test]
    fn opencode_receipt_rejects_mismatched_message_and_part_sessions() {
        let (_temp, db_path) = receipt_fixture();
        insert_user_part(
            &db_path,
            "foreign-message",
            "foreign-part",
            "ses-foreign",
            "ses-owned",
            "payload",
            "",
        );

        assert!(!opencode_database_contains_submitted_user_part(
            &db_path,
            0,
            "ses-owned",
            "payload"
        )
        .expect("session-bound query"));
    }

    #[test]
    fn opencode_receipt_baseline_fails_closed_when_schema_is_missing() {
        let temp = tempfile::tempdir().expect("fixture temp dir");
        let db_path = temp.path().join("opencode.db");
        rusqlite::Connection::open(&db_path).expect("empty fixture database");

        assert!(opencode_database_baseline_from_path(&db_path).is_err());
    }
}
