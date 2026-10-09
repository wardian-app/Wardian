//! Read-only recovery is correlated to a native MCP call, never the latest turn.
use super::*;
use wardian_core::agent_messaging::TaskContextCall;

const MAX_OBSERVED_CALLS: usize = 128;
const CALL_EVENT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub(super) struct TaskContextCalls {
    calls: HashMap<String, (String, bool)>,
    invalid: bool,
}

impl TaskContextCalls {
    pub(super) fn observe(&mut self, value: &Value, active_turn: Option<&str>) {
        let params = &value["params"];
        let item = &params["item"];
        if item["type"] != "mcpToolCall"
            || item["server"] != "wardian"
            || item["tool"] != "read_task_context"
        {
            return;
        }
        let (Some(id), Some(turn)) = (item["id"].as_str(), params["turnId"].as_str()) else {
            return;
        };
        if id.is_empty() || id.len() > 256 || Some(turn) != active_turn {
            return;
        }
        match value["method"].as_str() {
            Some("item/started") => {
                // Duplicate IDs cannot select a different call or revive a completed one.
                if self.calls.contains_key(id) || self.calls.len() >= MAX_OBSERVED_CALLS {
                    self.invalid = true;
                } else {
                    self.calls.insert(id.to_owned(), (turn.to_owned(), false));
                }
            }
            Some("item/completed") => {
                if let Some(call) = self.calls.get_mut(id) {
                    call.1 = true;
                }
            }
            _ => {}
        }
    }
}

/// Exact owner and native call evidence retained across the bounded lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskContextBinding {
    pub agent_id: String,
    pub generation: u64,
    pub thread_id: String,
    pub turn_id: String,
    pub provider_call: TaskContextCall,
}

fn error(code: &str, message: &str) -> CodexSharedError {
    CodexSharedError {
        code: code.into(),
        message: message.into(),
        provider_boundary_crossed: false,
    }
}

impl CodexSharedClient {
    fn task_context_binding_now(
        &self,
        call: &TaskContextCall,
    ) -> Result<Option<TaskContextBinding>, CodexSharedError> {
        let observation = self.observation.borrow();
        if !observation
            .task_context_policy
            .as_ref()
            .is_some_and(|policy| {
                policy.thread_id == call.thread_id && !policy.configuration_version.is_empty()
            })
        {
            return Err(error("task_context_unsupported", "Managed launch recovery policy is unqualified or was retired. No task snapshot was returned."));
        }
        if observation
            .provider_version
            .as_deref()
            .and_then(|version| version.split('+').next())
            != Some("0.160.0")
        {
            return Err(error(
                "task_context_unsupported",
                "Task recovery requires the qualified Codex 0.160.0 protocol.",
            ));
        }
        if observation.thread_id.as_deref() != Some(call.thread_id.as_str())
            || observation.closed
            || observation.stopped
        {
            return Err(error(
                "stale_task_context",
                "The native task-context owner or thread is no longer current.",
            ));
        }
        if observation.task_context_calls.invalid {
            return Err(error(
                "ambiguous_task_context",
                "Native task-context call evidence is ambiguous or exceeds its bound.",
            ));
        }
        let turn_id = match observation.activity() {
            CodexTurnActivity::Processing(turn) => turn,
            CodexTurnActivity::Pending | CodexTurnActivity::ProcessingWithoutTurn => {
                return Ok(None)
            }
            _ => {
                return Err(error(
                    "stale_task_context",
                    "Task recovery requires its originating active native turn.",
                ))
            }
        };
        let Some((observed_turn, completed)) =
            observation.task_context_calls.calls.get(&call.call_id)
        else {
            return Ok(None);
        };
        if *completed || observed_turn != &turn_id {
            return Err(error(
                "stale_task_context",
                "The originating native MCP call is no longer active.",
            ));
        }
        Ok(Some(TaskContextBinding {
            agent_id: self.agent_id.clone(),
            generation: self.generation,
            thread_id: call.thread_id.clone(),
            turn_id,
            provider_call: call.clone(),
        }))
    }

    /// Wait only for this call's native event. Pipe/event arrival order may differ;
    /// this never resends a provider call, admits work, or follows a new owner.
    pub(crate) async fn task_context_binding(
        &self,
        call: &TaskContextCall,
    ) -> Result<TaskContextBinding, CodexSharedError> {
        let fields = [&call.call_id, &call.thread_id, &call.reported_session_id];
        if fields
            .into_iter()
            .chain(call.originating_item_id.iter())
            .chain(call.window_id.iter())
            .any(|field| field.is_empty() || field.len() > 256 || field.trim() != field)
        {
            return Err(error(
                "invalid_task_context_call",
                "Native call metadata must contain bounded nonempty strings.",
            ));
        }
        let mut changes = self.observation.subscribe();
        let deadline = tokio::time::Instant::now() + CALL_EVENT_TIMEOUT;
        loop {
            changes.borrow_and_update();
            if let Some(binding) = self.task_context_binding_now(call)? {
                return Ok(binding);
            }
            if !matches!(
                tokio::time::timeout_at(deadline, changes.changed()).await,
                Ok(Ok(()))
            ) {
                return Err(error(
                    "task_context_call_unobserved",
                    "The exact native MCP call was not observed within the recovery deadline.",
                ));
            }
        }
    }

    /// Synchronous publication check; never substitutes a different active turn.
    pub(crate) fn validate_task_context_binding(
        &self,
        binding: &TaskContextBinding,
    ) -> Result<(), CodexSharedError> {
        if self
            .task_context_binding_now(&binding.provider_call)?
            .as_ref()
            != Some(binding)
        {
            return Err(error(
                "stale_task_context",
                "Native task-context evidence changed before response publication.",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
