//! Exact-turn terminal observations and attributed task results.

use super::*;
use crate::delivery::codex_shared::{CodexSharedClient, CodexSharedReceipt};
use crate::state::InteractionState;
use std::sync::Arc;

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

pub(super) struct TaskFinalDelivery {
    pub replies: Vec<store::Replied>,
    pub information: Vec<store::Admitted>,
}

pub(super) async fn observe_codex_task(
    interactions: &InteractionState,
    client: &CodexSharedClient,
    binding: &store::TaskTurnBinding,
    timeout: Duration,
) -> Result<(TaskFinalDelivery, String), ControlError> {
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
    if !matches!(status.as_str(), "completed" | "interrupted" | "failed") {
        interactions
            .mark_agent_task_turn_uncertain(binding)
            .await
            .map_err(control_error)?;
        return Err(ControlError::coded(
            "submitted_unconfirmed",
            "Codex did not report a recognized terminal task outcome.",
        ));
    }
    let (replies, information) = interactions
        .complete_agent_task_turn(binding, &status, &answer)
        .await
        .map_err(control_error)?;
    Ok((
        TaskFinalDelivery {
            replies,
            information,
        },
        answer,
    ))
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
            Ok((delivery, _)) => {
                for reply in &delivery.replies {
                    publish_completion(app.as_ref(), reply);
                }
                if let Some(app) = app.as_ref() {
                    for information in delivery.information {
                        for recipient in information.record.target_session_ids {
                            spawn_information(app, &recipient);
                        }
                    }
                }
            }
            Err(error) => manager::log_debug(&format!("[WARDIAN] task completion: {error}")),
        }
    });
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod behavior_tests;
