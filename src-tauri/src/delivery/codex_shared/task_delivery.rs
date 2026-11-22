//! Active tasks carry literal peer text as input and host-only routing as context.
use super::*;
use wardian_core::agent_messaging::AgentMessageContext;
use wardian_core::control::InteractionKind;

fn routing_context(frame: &AgentMessageContext) -> Result<Value, CodexSharedError> {
    let mut routing = serde_json::to_value(frame).map_err(|_| {
        CodexSharedError::unsupported("native task routing could not be serialized")
    })?;
    let fields = routing
        .as_object_mut()
        .expect("canonical context is an object");
    fields.remove("body");
    fields.remove("pending_information");
    // Only the host supplies application context. Peer text remains literal input.
    let key = uuid::Uuid::new_v4().to_string();
    Ok(json!({key: {
        "kind":"application", "value":routing.to_string()
    }}))
}

// A cancelled admission has no completion consumer to retire its receipt slot.
struct PendingTaskReceipt<'a> {
    client: &'a CodexSharedClient,
    request_id: &'a str,
    armed: bool,
}

impl Drop for PendingTaskReceipt<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.client.observation.send_modify(|state| {
                state.task_receipts.cancel(self.request_id);
            });
        }
    }
}

impl CodexSharedClient {
    pub(super) async fn admit_task(
        &self,
        message_id: &str,
        context: &str,
    ) -> Result<CodexSharedReceipt, CodexSharedError> {
        let frame: AgentMessageContext = serde_json::from_str(context).map_err(|_| {
            CodexSharedError::unsupported("native task requires canonical JSON AgentMessageContext")
        })?;
        if frame.schema_version != 1
            || !matches!(
                frame.kind,
                InteractionKind::Task | InteractionKind::Reply | InteractionKind::Message
            )
            || frame.interaction_id != message_id
            || match frame.kind {
                InteractionKind::Task => {
                    frame.request_id.as_deref() != Some(message_id)
                        || frame.parent_interaction_id.is_some()
                }
                InteractionKind::Reply => {
                    frame.parent_interaction_id.as_deref() != frame.request_id.as_deref()
                        || frame.reply_status.is_none()
                }
                InteractionKind::Message => {
                    frame.request_id.is_some() || frame.reply_status.is_some()
                }
                _ => true,
            }
        {
            return Err(CodexSharedError::unsupported(
                "native followup requires an exactly correlated canonical task or reply frame",
            ));
        }
        let reply_continuation = frame.kind == InteractionKind::Reply;
        let informational_message = frame.kind == InteractionKind::Message;
        let input_body = if reply_continuation {
            wardian_core::agent_messaging::reply_continuation_prompt(&frame)
        } else if informational_message {
            wardian_core::agent_messaging::information_delivery_prompt(&frame)
        } else {
            let information = wardian_core::agent_messaging::pending_information_prompt(&frame);
            if information.is_empty() {
                frame.body.clone()
            } else {
                format!("{information}\n\nWardian task:\n{}", frame.body)
            }
        };
        let mut receipt = self.receipt("provider_accepted")?;
        let activity = self.observation.borrow().activity();
        let (method, params, expected_turn) = match &activity {
            CodexTurnActivity::Processing(turn_id) if !turn_id.is_empty() => {
                if informational_message {
                    return Err(CodexSharedError::unsupported(
                        "informational messages use context injection while Codex is active",
                    ));
                }
                version::require_steer_version(receipt.provider_version.as_deref())?;
                ("turn/steer", json!({
                    "threadId": receipt.provider_session_id,
                    "clientUserMessageId": message_id,
                    "expectedTurnId": turn_id,
                    "input": [{"type":"text", "text":input_body}],
                    "additionalContext": routing_context(&frame)?,
                }), Some(turn_id.clone()))
            }
            CodexTurnActivity::Idle(_) | CodexTurnActivity::IdleWithoutTurn
                if frame.kind == InteractionKind::Task => (
                "turn/start", json!({
                    "threadId": receipt.provider_session_id,
                    "clientUserMessageId": message_id,
                    "input": [{"type":"text", "text":input_body}],
                    "additionalContext": routing_context(&frame)?,
                }), None,
            ),
            CodexTurnActivity::Idle(_) | CodexTurnActivity::IdleWithoutTurn => (
                "turn/start", json!({
                    "threadId": receipt.provider_session_id,
                    "input": [], "toolOutput": {
                        "name":if reply_continuation {"wardian_reply_continuation"} else {"wardian_inbox_delivery"},
                        "namespace":"wardian", "output":context
                    }
                }), None,
            ),
            _ => return Err(CodexSharedError::unsupported(
                "Codex task requires positively idle state or exact observed active turn; not written",
            )),
        };
        let task_input = frame.kind == InteractionKind::Task;
        if task_input {
            self.observation.send_modify(|state| {
                state.task_receipts.prepare(
                    message_id,
                    self.generation,
                    &receipt.provider_session_id,
                    &input_body,
                    &frame.body,
                )
            });
        }
        let mut pending_receipt = task_input.then(|| PendingTaskReceipt {
            client: self,
            request_id: message_id,
            armed: true,
        });
        let result = match self
            .request_with_activity(
                method,
                params,
                Some(Duration::from_secs(30)),
                Some(&activity),
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if task_input {
                    self.observation
                        .send_modify(|state| state.task_receipts.cancel(message_id));
                }
                return Err(error);
            }
        };
        let turn_id = if let Some(expected) = expected_turn {
            let returned = result["turnId"].as_str();
            if returned != Some(expected.as_str()) {
                return Err(CodexSharedError::uncertain(
                    "turn/steer did not acknowledge expectedTurnId; not replayed",
                ));
            }
            expected
        } else {
            result["turn"]["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    CodexSharedError::uncertain(
                        "turn/start returned no exact turn identity; not replayed",
                    )
                })?
                .to_owned()
        };
        if task_input {
            self.observation.send_modify(|state| {
                state
                    .task_receipts
                    .acknowledge(message_id, &result, &turn_id);
            });
            pending_receipt
                .as_mut()
                .expect("task receipt is prepared")
                .armed = false;
        }
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
