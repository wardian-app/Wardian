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
}
