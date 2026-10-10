use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChatEventKind {
    Message,
    ToolCall,
    ToolResult,
    Approval,
    Status,
    TerminalOutput,
    Memory,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChatRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChatStatus {
    Running,
    Succeeded,
    Failed,
    ActionRequired,
    Cancelled,
    Idle,
    Processing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentChatEvent {
    pub id: String,
    pub session_id: String,
    pub provider: String,
    pub kind: AgentChatEventKind,
    pub role: Option<AgentChatRole>,
    pub text: Option<String>,
    pub title: Option<String>,
    pub status: Option<AgentChatStatus>,
    pub turn_id: Option<String>,
    pub source: Option<String>,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub path: Option<String>,
    pub language: Option<String>,
    pub created_at: Option<String>,
    pub sequence: Option<u64>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Bounded display headers shared by desktop and the authenticated remote API.
/// Cursors and detail references are opaque and pinned to immutable objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatPage {
    pub session_id: String,
    pub conversation_id: Option<String>,
    pub generation: Option<String>,
    pub source_epoch: Option<String>,
    pub revision: String,
    pub events: Vec<AgentChatEvent>,
    pub next_before: Option<String>,
    pub unchanged: bool,
    pub reset: bool,
    pub progress: String,
    pub aliases: Vec<AgentChatAlias>,
    pub removed_ids: Vec<String>,
    pub detail: Option<AgentChatDetail>,
    pub bytes_read: usize,
    pub records_decoded: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatAlias {
    pub observation_id: String,
    pub canonical_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatDetail {
    pub event_id: String,
    pub text: String,
    pub next: Option<String>,
    pub complete: bool,
}

/// Optional proof of a generated input row committed by the archive owner.
/// Provider acceptance alone cannot produce this receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatInputReceipt {
    pub chat_event_id: String,
    pub chat_agent_id: String,
    pub chat_conversation_id: String,
    pub chat_source_epoch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatPromptDeliveryDetail {
    #[serde(flatten)]
    pub delivery: crate::control::DeliveryDetail,
    #[serde(flatten)]
    pub chat_receipt: Option<ChatInputReceipt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_chat_event_serializes_snake_case() {
        let event = AgentChatEvent {
            id: "agent-1:1".to_string(),
            session_id: "agent-1".to_string(),
            provider: "codex".to_string(),
            kind: AgentChatEventKind::ToolCall,
            role: None,
            text: None,
            title: Some("Shell".to_string()),
            status: Some(AgentChatStatus::Running),
            turn_id: Some("turn-1".to_string()),
            source: Some("response_item".to_string()),
            command: Some("npm run lint".to_string()),
            exit_code: None,
            path: None,
            language: Some("shell".to_string()),
            created_at: Some("2026-05-21T00:00:00.000Z".to_string()),
            sequence: Some(1),
            metadata: serde_json::json!({"provider_type":"exec_command"}),
        };

        let json = serde_json::to_value(&event).expect("serialize event");

        assert_eq!(json["kind"], "tool_call");
        assert_eq!(json["status"], "running");
        assert_eq!(json["session_id"], "agent-1");
        assert_eq!(json["turn_id"], "turn-1");
        assert_eq!(json["exit_code"], serde_json::Value::Null);
    }

    #[test]
    fn chat_receipt_is_optional_and_preserves_the_delivery_payload() {
        let delivery: crate::control::DeliveryDetail = serde_json::from_value(serde_json::json!({
            "uuid": "agent", "name": "Agent", "provider": "codex", "runtime_state": "running", "delivery_state": "provider_accepted"
        })).unwrap();
        let original = serde_json::to_value(&delivery).unwrap();
        let absent = serde_json::to_value(ChatPromptDeliveryDetail {
            delivery: delivery.clone(),
            chat_receipt: None,
        })
        .unwrap();
        assert_eq!(absent, original);
        assert!(serde_json::from_value::<ChatPromptDeliveryDetail>(absent)
            .unwrap()
            .chat_receipt
            .is_none());
        let receipt = ChatInputReceipt {
            chat_event_id: "generated:conversation:1".into(),
            chat_agent_id: "agent".into(),
            chat_conversation_id: "conversation".into(),
            chat_source_epoch: None,
        };
        let present = serde_json::to_value(ChatPromptDeliveryDetail {
            delivery,
            chat_receipt: Some(receipt.clone()),
        })
        .unwrap();
        assert_eq!(present["chat_source_epoch"], serde_json::Value::Null);
        assert_eq!(present["delivery_state"], "provider_accepted");
        assert_eq!(
            serde_json::from_value::<ChatPromptDeliveryDetail>(present)
                .unwrap()
                .chat_receipt,
            Some(receipt)
        );
    }
}
