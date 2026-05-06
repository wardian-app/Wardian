use crate::identity::AgentIdentity;
use serde::{Deserialize, Serialize};
pub const CONTROL_SCHEMA: u8 = 1;
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ControlRequest {
    AgentList,
    AgentKill { target: String },
    AgentPause { target: String },
    AgentResume { target: String },
    AgentSpawn { class: String, name: Option<String>, workspace: Option<String> },
    AgentClone { target: String, name: Option<String> },
    WorkflowList,
    WorkflowRun { id: String },
    WorkflowStop { run_instance_id: String },
    SendMessage { target: String, message: String, thread: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentListResponse {
    pub schema: u8,
    pub agents: Vec<AgentIdentity>,
}

impl AgentListResponse {
    pub fn new(agents: Vec<AgentIdentity>) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            agents,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OkResponse {
    pub schema: u8,
    pub ok: bool,
}

impl OkResponse {
    pub fn new() -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            ok: true,
        }
    }
}

impl Default for OkResponse {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentResponse {
    pub schema: u8,
    pub agent: AgentIdentity,
}

impl AgentResponse {
    pub fn new(agent: AgentIdentity) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            agent,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowSummary {
    pub id: String,
    pub name: String,
    pub node_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowListResponse {
    pub schema: u8,
    pub workflows: Vec<WorkflowSummary>,
}

impl WorkflowListResponse {
    pub fn new(workflows: Vec<WorkflowSummary>) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            workflows,
        }
    }
}

pub fn endpoint_key() -> Option<String> {
    let home = crate::paths::wardian_home()?;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in home.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    Some(format!("{hash:016x}"))
}

#[cfg(windows)]
pub fn pipe_name() -> Option<String> {
    endpoint_key().map(|key| format!(r"\\.\pipe\wardian-control-{key}"))
}

#[cfg(unix)]
pub fn socket_path() -> Option<std::path::PathBuf> {
    crate::paths::wardian_home().map(|home| home.join("run").join("control.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_response_uses_current_schema() {
        let response = AgentListResponse::new(Vec::new());
        assert_eq!(response.schema, CONTROL_SCHEMA);
    }

    #[test]
    fn endpoint_key_is_stable_for_home_path() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_HOME", "D:/Development/Wardian/.tmp/live-status");
        assert_eq!(endpoint_key().as_deref(), Some("50403db71b810eba"));
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn agent_kill_request_serializes_with_target() {
        let req = ControlRequest::AgentKill { target: "coder-a1".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""command":"agent_kill""#));
        assert!(json.contains(r#""target":"coder-a1""#));
    }

    #[test]
    fn ok_response_serializes() {
        let resp = OkResponse::new();
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""ok":true"#));
        assert!(json.contains(r#""schema":1"#));
    }

    #[test]
    fn send_message_request_serializes() {
        let req = ControlRequest::SendMessage {
            target: "all".to_string(),
            message: "hello".to_string(),
            thread: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""command":"send_message""#));
        assert!(json.contains(r#""target":"all""#));
    }

    #[test]
    fn workflow_summary_node_count_roundtrips() {
        let summary = WorkflowSummary {
            id: "wf-1".to_string(),
            name: "My Flow".to_string(),
            node_count: 3,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let back: WorkflowSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.node_count, 3);
    }
}
