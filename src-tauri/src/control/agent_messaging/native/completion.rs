//! Exact-turn fallback for canonical Codex tasks, separate from human input.

use super::*;
use crate::delivery::codex_shared::{CodexSharedClient, CodexSharedReceipt};
use crate::state::InteractionState;
use std::sync::Arc;
use wardian_core::control::ReplyStatus;

pub(super) async fn bind_codex_task(
    state: &AppState,
    claim: &store::TaskClaim,
    receipt: &CodexSharedReceipt,
) -> Result<store::TaskTurnBinding, ControlError> {
    let binding = async {
        let recipient = claim.record.target_session_ids.first();
        if recipient.map(String::as_str) != Some(receipt.wardian_agent_id.as_str())
            || receipt.generation != claim.generation
            || receipt.message_id.as_deref() != Some(claim.record.id.as_str())
        {
            return Err(ControlError::coded(
                "submitted_unconfirmed",
                "Codex acknowledgement did not identify this task and runtime.",
            ));
        }
        let turn = receipt.provider_turn_id.as_deref().ok_or_else(|| {
            ControlError::coded(
                "submitted_unconfirmed",
                "Codex returned no task turn identity.",
            )
        })?;
        let mode = receipt.admission_mode.as_deref().ok_or_else(|| {
            ControlError::coded(
                "submitted_unconfirmed",
                "Codex returned no task admission mode.",
            )
        })?;
        state
            .interactions
            .bind_agent_task_turn(claim, &receipt.provider_session_id, turn, mode)
            .await
            .map_err(control_error)
    }
    .await;
    if binding.is_err() {
        // Acceptance without a durable exact binding is never replayable.
        let _ = state
            .interactions
            .finish_agent_task(claim, "uncertain")
            .await;
    }
    binding
}

/// Provider terminal evidence determines status; text never determines routing.
fn terminal_reply(status: &str, answer: &str) -> Option<(ReplyStatus, String)> {
    match status {
        "completed" if answer.trim().is_empty() => Some((
            ReplyStatus::Blocked,
            "Wardian: the task's bound turn completed without usable final answer text. Use an explicit reply to supply a result before turn completion when needed.".into(),
        )),
        "completed" if answer.len() > wardian_core::agent_messaging::MAX_MESSAGE_BYTES => Some((
            ReplyStatus::Blocked,
            "Wardian: the task's bound turn completed, but its final answer exceeds the 64 KiB reply limit. Automatic publication cannot supply that result.".into(),
        )),
        "completed" => Some((ReplyStatus::Done, answer.to_owned())),
        "interrupted" => Some((
            ReplyStatus::Blocked,
            "Wardian: the provider interrupted the task's bound turn before completion.".into(),
        )),
        "failed" => Some((
            ReplyStatus::Failed,
            "Wardian: the provider reported failure for the task's bound turn.".into(),
        )),
        _ => None,
    }
}

pub(super) async fn observe_codex_task(
    interactions: &InteractionState,
    client: &CodexSharedClient,
    binding: &store::TaskTurnBinding,
    timeout: Duration,
) -> Result<(Option<store::Replied>, String), ControlError> {
    let result = client
        .wait_for_final_result(&binding.provider_turn_id, timeout)
        .await;
    let (status, answer) = match result {
        Ok(result) => result,
        Err(error) => {
            interactions
                .mark_agent_task_turn_uncertain(binding)
                .await
                .map_err(control_error)?;
            return Err(ControlError::coded("submitted_unconfirmed", error.message));
        }
    };
    let Some((status, body)) = terminal_reply(&status, &answer) else {
        interactions
            .mark_agent_task_turn_uncertain(binding)
            .await
            .map_err(control_error)?;
        return Err(ControlError::coded(
            "submitted_unconfirmed",
            "Codex did not report a recognized terminal task outcome.",
        ));
    };
    let reply = interactions
        .complete_agent_task_turn(binding, status, &body)
        .await
        .map_err(control_error)?;
    Ok((reply, answer))
}

pub(super) fn publish_completion(app: Option<&AppHandle>, replied: &store::Replied) {
    if let Some(app) = app {
        let _ = app.emit("pair-activity-changed", ());
        for recipient in &replied.record.target_session_ids {
            spawn_information(app, recipient);
        }
    }
}

pub(super) fn spawn_codex_task_completion(
    app: Option<AppHandle>,
    interactions: Arc<InteractionState>,
    client: Arc<CodexSharedClient>,
    binding: store::TaskTurnBinding,
) {
    tokio::spawn(async move {
        match observe_codex_task(&interactions, &client, &binding, Duration::from_secs(900)).await {
            Ok((Some(reply), _)) => publish_completion(app.as_ref(), &reply),
            Ok((None, _)) => {}
            Err(error) => manager::log_debug(&format!("[WARDIAN] task completion: {error}")),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_evidence_preserves_final_text_and_bounds_fallback() {
        let body = "\r\n final café 日本語 \r\n";
        assert_eq!(
            terminal_reply("completed", body),
            Some((ReplyStatus::Done, body.into()))
        );
        assert_eq!(
            terminal_reply("completed", "  \n").unwrap().0,
            ReplyStatus::Blocked
        );
        assert_eq!(
            terminal_reply("completed", &"x".repeat(65_537)).unwrap().0,
            ReplyStatus::Blocked
        );
        assert_eq!(
            terminal_reply("interrupted", "partial").unwrap().0,
            ReplyStatus::Blocked
        );
        assert_eq!(
            terminal_reply("failed", "partial").unwrap().0,
            ReplyStatus::Failed
        );
        assert!(terminal_reply("unknown", "plausible answer").is_none());
    }
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod behavior_tests;
