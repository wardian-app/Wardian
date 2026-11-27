//! Prompt delivery entry points and optional display receipts.
use super::message_delivery::DeliveryContext;
use super::*;

/// Preserve the delivery result when optional display persistence fails. The
/// generated-row receipt is best effort and can never invite a second submit.
pub(crate) async fn deliver_prompt_to_agent_with_chat_receipt(
    app: Option<&AppHandle>,
    state: &AppState,
    target: &str,
    prompt: &str,
    input_mode: MessageInputMode,
    chat_composer: bool,
    terminal_attachments: bool,
) -> Result<wardian_core::models::chat::ChatPromptDeliveryDetail, ControlError> {
    // Chat's transcript waits for an owned provider row. A generated archive
    // receipt only proves Wardian stored the submitted text.
    let before = (!chat_composer)
        .then(|| crate::commands::chat::chat_read_snapshot_for_state(state, target).ok())
        .flatten();
    let before_fence = before.as_ref().and_then(|before| {
        let (global, _) = crate::utils::shell::recent_conversation_logging().ok()?;
        if effective_conversation_logging(global, before.0.agent_conversation_logging)
            != ConversationLoggingSetting::Enabled
        {
            return None;
        }
        state
            .conversation_archive
            .archive_input_fence(
                &crate::commands::chat::conversation_archive_context_from_snapshot(&before.0),
                before.0.log_path.as_deref(),
            )
            .ok()
            .flatten()
    });
    let delivery = deliver_message_to_target_with_delivery_options(
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
        None,
        None,
        None,
        DeliveryContext {
            chat_composer: chat_composer && !terminal_attachments,
            ..Default::default()
        },
    )
    .await?;
    let detail = delivery.into_iter().next().ok_or_else(|| {
        ControlError::request_failed("prompt delivery produced no result".to_string())
    })?;
    let mut chat_receipt = None;
    if before_fence.is_some()
        && detail.message_id.is_none()
        && detail.error.is_none()
        && matches!(
            detail.delivery_state.as_str(),
            "submitted" | "provider_accepted" | "approval_submitted"
        )
    {
        if let Ok(_policy) = state.conversation_capture_policy_lock.try_lock() {
            if let Ok(snapshot) =
                crate::commands::chat::chat_read_snapshot_for_state(state, &detail.uuid)
            {
                let logging = crate::utils::shell::recent_conversation_logging();
                let same_runtime = before.as_ref().is_some_and(|before| {
                    Arc::ptr_eq(&before.1, &snapshot.1)
                        && crate::commands::chat::conversation_archive_context_from_snapshot(
                            &before.0,
                        )
                        .provider_source_key
                            == crate::commands::chat::conversation_archive_context_from_snapshot(
                                &snapshot.0,
                            )
                            .provider_source_key
                });
                if same_runtime
                    && logging.is_ok_and(|(global, _)| {
                        effective_conversation_logging(
                            global,
                            snapshot.0.agent_conversation_logging,
                        ) == ConversationLoggingSetting::Enabled
                    })
                {
                    let context = crate::commands::chat::conversation_archive_context_from_snapshot(
                        &snapshot.0,
                    );
                    let after_fence = state
                        .conversation_archive
                        .archive_input_fence(&context, snapshot.0.log_path.as_deref())
                        .ok()
                        .flatten();
                    if let Some(fence) =
                        before_fence.filter(|fence| Some(fence) == after_fence.as_ref())
                    {
                        match state
                            .conversation_archive
                            .append_delivered_input_receipt(context, prompt, &fence)
                        {
                            Ok(receipt) => chat_receipt = receipt,
                            Err(_) => manager::log_debug(
                                "[WARDIAN] optional Chat input receipt unavailable after delivery",
                            ),
                        }
                    }
                }
            }
        }
    }
    Ok(wardian_core::models::chat::ChatPromptDeliveryDetail {
        delivery: detail,
        chat_receipt,
    })
}

/// Deliver a remote-client message through the queued control path.
///
/// Chat attachments supply the app handle to identify the composer and to
/// keep its mod observer alive after the HTTP request ends.
pub(crate) async fn deliver_remote_client_message(
    state: &AppState,
    target: &str,
    message: &str,
    interaction_id: &str,
    deadline_at: &str,
    authority: crate::remote::client_messages::ClientSendAuthority,
    chat_app: Option<&AppHandle>,
) -> Result<Vec<DeliveryDetail>, ControlError> {
    let orchestration = wardian_core::control::OrchestrationDeliveryOptions {
        idempotency_key: Some(interaction_id.to_string()),
        deadline_at: Some(deadline_at.to_string()),
        ..Default::default()
    };
    deliver_message_to_target_with_delivery_options(
        chat_app,
        state,
        target,
        message,
        None,
        MessageInputMode::Message,
        QueuePolicy::QueueIfBusy,
        None,
        None,
        false,
        crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
        Some(&orchestration),
        None,
        Some(interaction_id),
        DeliveryContext {
            client_send_authority: Some(authority),
            chat_composer: chat_app.is_some(),
        },
    )
    .await
}
