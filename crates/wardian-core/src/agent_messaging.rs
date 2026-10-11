//! Receiver-first agent messaging. Interaction records own all message bodies;
//! availability references and cursors only govern delivery and acknowledgement.

use crate::control::{InteractionKind, ReplyStatus};
use serde::{Deserialize, Serialize};

mod task_outcomes;
pub use task_outcomes::{
    parse_task_outcomes, ParsedTaskOutcomes, TaskOutcome, TaskOutcomePacket, MAX_TASK_OUTCOMES,
    TASK_OUTCOME_CLOSE, TASK_OUTCOME_INSTRUCTIONS, TASK_OUTCOME_OPEN,
};

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// Aggregate literal message bytes in one receive page.
pub const MAX_RECEIVE_BODY_BYTES: usize = 128 * 1024;
/// Serialized response bound, allowing JSON escaping of one maximum-size body.
pub const MAX_RECEIVE_SERIALIZED_BYTES: usize = 512 * 1024;
pub const MAX_RECEIVE_ITEMS: u32 = 100;
pub const MAX_MAILBOX_WAIT_TIMEOUT_MS: u64 = 60_000;
pub const MAX_RECEIVE_TIMEOUT_MS: u64 = MAX_MAILBOX_WAIT_TIMEOUT_MS;
pub const MAX_WAIT_AGENT_TIMEOUT_MS: u64 = MAX_MAILBOX_WAIT_TIMEOUT_MS;
/// Bound the complete MCP result, including its wrapper and source labels.
pub const MAX_TASK_CONTEXT_RESULT_BYTES: usize = 4096;
/// Codex 0.160 applies a four-byte threshold per configured output token.
pub const TASK_CONTEXT_OUTPUT_TOKENS: usize = 2048;

/// Provider transport metadata, never model-facing tool arguments. These fields
/// are untrusted until matched to the current owner's native MCP call event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskContextCall {
    pub call_id: String,
    pub thread_id: String,
    pub reported_session_id: String,
    pub originating_item_id: Option<String>,
    pub window_id: Option<String>,
}

/// Literal canonical peer text. Sequence and timestamp describe inbox/request
/// chronology; neither establishes precedence over human instructions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveredTaskContext {
    pub availability_sequence: i64,
    pub created_at: String,
    pub message: AgentMessageContext,
}

/// Managed-origin operations; caller identity is supplied outside this payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMessagingRequest {
    ListAgents,
    ReadTaskContext {
        provider_call: TaskContextCall,
    },
    SendMessage {
        target: String,
        message: String,
        #[serde(default)]
        idempotency_key: Option<String>,
    },
    FollowupTask {
        target: String,
        message: String,
        #[serde(default)]
        idempotency_key: Option<String>,
    },
    ReceiveMessages {
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        ack_cursor: Option<String>,
        #[serde(default)]
        limit: Option<u32>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    WaitAgent {
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    Reply {
        request_id: String,
        status: ReplyStatus,
        message: String,
    },
    InterruptAgent {
        target: String,
    },
}

/// Exactly one owner can expose a task: the receiver or the prompt scheduler.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskDeliveryOwner {
    Unclaimed,
    Receiver,
    Scheduler,
}

/// Trusted host attribution, separate from registered agent identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostAutomationProvenance {
    pub run_id: String,
    pub node: String,
}

/// A projection of one canonical interaction available to this recipient.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMessage {
    pub interaction_id: String,
    pub kind: InteractionKind,
    pub sender: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_automation: Option<HostAutomationProvenance>,
    pub message: String,
    pub parent_interaction_id: Option<String>,
    pub reply_status: Option<ReplyStatus>,
    pub created_at: String,
}

/// A replayable page. Only an explicit subsequent `ack_cursor` advances the
/// acknowledged highwater. Cursor tokens are opaque, versioned and recipient-bound.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMessagePage {
    pub messages: Vec<AgentMessage>,
    pub next_cursor: String,
    pub ack_cursor: String,
    pub has_more: bool,
    pub timed_out: bool,
    /// A delivery committed during this wait on the exclusive provider path.
    /// This is not body consumption, task completion, or cursor acknowledgement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_reason: Option<String>,
}

/// Provider-neutral host-delivery frame. `body` is canonical literal text;
/// routing and reply correlation never get concatenated into that text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMessageContext {
    pub schema_version: u8,
    pub sender: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_automation: Option<HostAutomationProvenance>,
    pub recipient: String,
    pub kind: InteractionKind,
    pub interaction_id: String,
    pub parent_interaction_id: Option<String>,
    pub request_id: Option<String>,
    pub body: String,
    pub reply_status: Option<ReplyStatus>,
}

/// Admission is distinct from execution, model consumption, and task completion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AgentMessagingResponse {
    ReadTaskContext {
        agent_id: String,
        generation: u64,
        thread_id: String,
        turn_id: String,
        provider_call: TaskContextCall,
        observed_at: String,
        priority: String,
        chronology: String,
        tasks: Vec<RecoveredTaskContext>,
    },
    InterruptAgent {
        target_agent_id: String,
        generation: u64,
        delivery_state: String,
        interruption_confirmed: bool,
        provider_session_id: String,
        provider_turn_id: Option<String>,
    },
    ListAgents {
        agents: Vec<crate::identity::AgentIdentity>,
    },
    SendMessage {
        interaction_id: String,
        delivery_state: String,
        duplicate: bool,
    },
    FollowupTask {
        request_id: String,
        delivery_state: String,
        delivery_owner: TaskDeliveryOwner,
        duplicate: bool,
    },
    ReceiveMessages {
        #[serde(flatten)]
        page: AgentMessagePage,
    },
    WaitAgent {
        timed_out: bool,
    },
    Reply {
        request_id: String,
        interaction_id: String,
        delivery_state: String,
        duplicate: bool,
    },
}

/// Stable machine-readable failure across DB, control, and tool adapters.
#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct AgentMessagingError {
    pub code: String,
    pub message: String,
}

impl AgentMessagingError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Build and bound the actual MCP recovery result, including both projections.
/// Never truncate literal task text or return a partial list to satisfy a limit.
/// The supported Codex consumer selects structured content for model input.
/// Keep one canonical payload there; a second full text copy consumes the
/// recovery budget without supplying additional task context.
pub fn task_context_mcp_result(
    mut value: serde_json::Value,
    is_error: bool,
) -> Result<serde_json::Value, AgentMessagingError> {
    let text = if is_error {
        value.to_string()
    } else {
        "Task context is available in structuredContent.".into()
    };
    if !is_error {
        value["task_outcome_instructions"] = serde_json::json!(TASK_OUTCOME_INSTRUCTIONS);
    }
    let result = serde_json::json!({
        "content":[{"type":"text","text":text}],
        "structuredContent":value,"isError":is_error,
    });
    if result.to_string().len() > MAX_TASK_CONTEXT_RESULT_BYTES {
        return Err(AgentMessagingError::new("task_context_overflow", "The complete task-context result exceeds the supported output bound. No partial task list was returned."));
    }
    Ok(result)
}

impl From<rusqlite::Error> for AgentMessagingError {
    fn from(error: rusqlite::Error) -> Self {
        Self::new("storage_error", error.to_string())
    }
}

#[cfg(test)]
mod tests;
