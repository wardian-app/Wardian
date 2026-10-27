//! Prompt delivery entry points and optional display receipts.
use super::*;

/// Preserve the delivery result when optional display persistence fails. The
/// generated-row receipt is best effort and can never invite a second submit.
pub(crate) async fn deliver_prompt_to_agent_with_chat_receipt(
    app: Option<&AppHandle>,
    state: &AppState,
    target: &str,
    prompt: &str,
    input_mode: MessageInputMode,
) -> Result<wardian_core::models::chat::ChatPromptDeliveryDetail, ControlError> {
    let before = crate::commands::chat::chat_read_snapshot_for_state(state, target).ok();
    let before_fence = before.as_ref().and_then(|before| {
        crate::state::conversation_archive::chat_read::input_fence(
            &crate::commands::chat::conversation_archive_context_from_snapshot(&before.0),
            before.0.log_path.as_deref(),
        )
        .ok()
        .flatten()
    });
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
    let detail = delivery.into_iter().next().ok_or_else(|| {
        ControlError::request_failed("prompt delivery produced no result".to_string())
    })?;
    let mut chat_receipt = None;
    if detail.message_id.is_none()
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
                let global = crate::utils::shell::load_shell_settings()
                    .unwrap_or_default()
                    .conversation_logging;
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
                    && effective_conversation_logging(global, snapshot.0.agent_conversation_logging)
                        == ConversationLoggingSetting::Enabled
                {
                    let context = crate::commands::chat::conversation_archive_context_from_snapshot(
                        &snapshot.0,
                    );
                    let after_fence = crate::state::conversation_archive::chat_read::input_fence(
                        &context,
                        snapshot.0.log_path.as_deref(),
                    )
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
