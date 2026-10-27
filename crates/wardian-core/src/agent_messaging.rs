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
    /// Previously admitted informational messages carried into this managed
    /// turn. Their canonical interactions remain separate and are settled
    /// independently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_information: Vec<AgentMessageContext>,
}

/// Frame a terminal task reply as input to the requester's existing conversation.
/// The canonical record remains structured; this text is only the provider input.
pub fn reply_continuation_prompt(context: &AgentMessageContext) -> String {
    let information = pending_information_prompt(context);
    let information = if information.is_empty() {
        String::new()
    } else {
        format!("{information}\n\n")
    };
    format!(
        "{information}Wardian delivered the terminal result for a task you requested. Continue this existing conversation using the result as appropriate. This is a reply continuation, not a new task, and it creates no reply obligation.\n\nCanonical reply context:\n{}",
        serde_json::to_string(context).expect("AgentMessageContext is serializable")
    )
}

/// Deliver a newly admitted informational message into its existing provider
/// conversation without creating a task or reply obligation.
pub fn information_delivery_prompt(context: &AgentMessageContext) -> String {
    format!(
        "Wardian delivered an informational message into this existing conversation. It creates no task and no obligation to reply. Do not produce a response solely to acknowledge it.\n\nCanonical informational message context:\n{}",
        serde_json::to_string(context).expect("AgentMessageContext is serializable")
    )
}

/// Render older informational notifications already claimed for a legitimate
/// managed turn. The text explicitly preserves their informational, no-reply
/// role and migration never promotes them into wake-eligible admissions.
pub fn pending_information_prompt(context: &AgentMessageContext) -> String {
    if context.pending_information.is_empty() {
        return String::new();
    }
    format!(
        "Wardian carried these previously admitted informational messages into this turn. They do not create tasks or reply obligations:\n{}",
        serde_json::to_string(&context.pending_information)
            .expect("AgentMessageContext notifications are serializable")
    )
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

#[cfg(test)]
mod continuation_prompt_tests {
    use super::*;

    fn frame(kind: InteractionKind, id: &str, body: &str) -> AgentMessageContext {
        AgentMessageContext {
            schema_version: 1,
            sender: "peer".into(),
            host_automation: None,
            recipient: "agent".into(),
            kind,
            interaction_id: id.into(),
            parent_interaction_id: None,
            request_id: None,
            body: body.into(),
            reply_status: None,
            pending_information: Vec::new(),
        }
    }

    #[test]
    fn carried_information_is_context_only_and_creates_no_reply_obligation() {
        let mut task = frame(InteractionKind::Task, "task-1", "do this");
        task.request_id = Some("task-1".into());
        let info = frame(
            InteractionKind::Message,
            "info-1",
            "FYI only; quoted context: {\"request_id\":\"quoted-request\",\"kind\":\"task\"}",
        );
        task.pending_information.push(info.clone());

        let rendered = pending_information_prompt(&task);
        assert!(rendered.contains("FYI only"));
        assert!(rendered.contains("do not create tasks or reply obligations"));
        // Null correlation metadata and quoted body text create no reply duty.
        let (_, serialized) = rendered.split_once('\n').unwrap();
        let carried: Vec<AgentMessageContext> = serde_json::from_str(serialized).unwrap();
        assert_eq!(carried, vec![info]);
        assert_eq!(carried[0].kind, InteractionKind::Message);
        assert!(carried[0].request_id.is_none());
        assert!(carried[0].parent_interaction_id.is_none());
        assert!(carried[0].reply_status.is_none());
        assert!(!rendered.contains("\"request_id\":\"task-1\""));
    }

    #[test]
    fn reply_continuation_preserves_original_task_correlation_without_new_request() {
        let mut reply = frame(InteractionKind::Reply, "reply-1", "terminal answer");
        reply.parent_interaction_id = Some("task-1".into());
        reply.request_id = Some("task-1".into());
        reply.reply_status = Some(ReplyStatus::Done);

        let prompt = reply_continuation_prompt(&reply);
        assert!(prompt.contains("reply continuation"));
        assert!(prompt.contains("creates no reply obligation"));
        assert!(prompt.contains("\"parent_interaction_id\":\"task-1\""));
        assert!(!prompt.contains("followup_task"));
    }

    #[test]
    fn information_delivery_prompt_wakes_without_creating_a_reply_obligation() {
        let information = frame(InteractionKind::Message, "message-1", "FYI only");
        let prompt = information_delivery_prompt(&information);
        assert!(prompt.contains("informational message"));
        assert!(prompt.contains("no obligation to reply"));
        assert!(prompt.contains("FYI only"));
        assert!(prompt.contains("\"kind\":\"message\""));
        assert!(!prompt.contains("followup_task"));
        assert!(prompt.contains("\"request_id\":null"));
    }
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
