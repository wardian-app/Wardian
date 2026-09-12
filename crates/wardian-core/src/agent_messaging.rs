//! Receiver-first agent messaging. Interaction records own all message bodies;
//! availability references and cursors only govern delivery and acknowledgement.

use crate::control::{InteractionKind, ReplyStatus};
use serde::{Deserialize, Serialize};

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// Aggregate literal message bytes in one receive page.
pub const MAX_RECEIVE_BODY_BYTES: usize = 128 * 1024;
/// Serialized response bound, allowing JSON escaping of one maximum-size body.
pub const MAX_RECEIVE_SERIALIZED_BYTES: usize = 512 * 1024;
pub const MAX_RECEIVE_ITEMS: u32 = 100;
pub const MAX_RECEIVE_TIMEOUT_MS: u64 = 60_000;

/// Managed-origin operations; caller identity is supplied outside this payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMessagingRequest {
    ListAgents,
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

/// A projection of one canonical interaction available to this recipient.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMessage {
    pub interaction_id: String,
    pub kind: InteractionKind,
    pub sender: String,
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

impl From<rusqlite::Error> for AgentMessagingError {
    fn from(error: rusqlite::Error) -> Self {
        Self::new("storage_error", error.to_string())
    }
}
