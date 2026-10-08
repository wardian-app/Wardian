//! Active tasks carry literal peer text as input and host-only routing as context.
use super::*;
use wardian_core::agent_messaging::AgentMessageContext;
use wardian_core::control::InteractionKind;

impl CodexSharedClient {
    pub(super) async fn admit_task(
        &self,
        message_id: &str,
        context: &str,
    ) -> Result<CodexSharedReceipt, CodexSharedError> {
        let frame: AgentMessageContext = serde_json::from_str(context).map_err(|_| {
            CodexSharedError::unsupported("native task requires canonical JSON AgentMessageContext")
        })?;
        if frame.schema_version != 1 || frame.kind != InteractionKind::Task {
            return Err(CodexSharedError::unsupported(
                "native followup requires a canonical task frame",
            ));
        }
        let mut receipt = self.receipt("provider_accepted")?;
        let activity = self.observation.borrow().activity();
        let (method, params, expected_turn) = match &activity {
            CodexTurnActivity::Processing(turn_id) if !turn_id.is_empty() => {
                version::require_steer_version(receipt.provider_version.as_deref())?;
                let mut routing = serde_json::to_value(&frame).map_err(|_| {
                    CodexSharedError::unsupported("native task routing could not be serialized")
                })?;
                routing.as_object_mut().unwrap().remove("body");
                // The key is opaque to Codex. Only the host constructs this metadata;
                // peer text must never become trusted application instructions.
                let key = uuid::Uuid::new_v4().to_string();
                ("turn/steer", json!({
                    "threadId": receipt.provider_session_id,
                    "expectedTurnId": turn_id,
                    "input": [{"type":"text", "text":frame.body}],
                    "additionalContext": {key: {"kind":"application", "value":routing.to_string()}},
                }), Some(turn_id.clone()))
            }
            CodexTurnActivity::Idle(_) | CodexTurnActivity::IdleWithoutTurn => (
                "turn/start", json!({
                    "threadId": receipt.provider_session_id,
                    "input": [], "toolOutput": {
                        "name":"wardian_task_delivery", "namespace":"wardian", "output":context
                    }
                }), None,
            ),
            _ => return Err(CodexSharedError::unsupported(
                "Codex task requires positively idle state or exact observed active turn; not written",
            )),
        };
        let task_admission = expected_turn.as_deref().map_or(
            TaskAdmission::IdleStart(message_id),
            TaskAdmission::ActiveSteer,
        );
        let result = self
            .request_with_task_admission(
                method,
                params,
                Some(Duration::from_secs(30)),
                Some(&activity),
                task_admission,
            )
            .await?;
        let turn_id = if let Some(expected) = expected_turn {
            let returned = result["turnId"].as_str();
            if returned != Some(expected.as_str()) {
                self.abandon_task_start(message_id);
                return Err(CodexSharedError::uncertain(
                    "turn/steer did not acknowledge expectedTurnId; not replayed",
                ));
            }
            expected
        } else {
            let Some(turn_id) = result["turn"]["id"].as_str().filter(|id| !id.is_empty()) else {
                self.abandon_task_start(message_id);
                return Err(CodexSharedError::uncertain(
                    "turn/start returned no exact turn identity; not replayed",
                ));
            };
            let turn_id = turn_id.to_owned();
            self.confirm_task_start(message_id, &turn_id);
            turn_id
        };
        receipt.provider_turn_id = Some(turn_id);
        receipt.message_id = Some(message_id.to_owned());
        receipt.admission_mode = Some(
            if method == "turn/steer" {
                "steer"
            } else {
                "start"
            }
            .into(),
        );
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests;
