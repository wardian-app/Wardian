use crate::identity::AgentIdentity;
use crate::models::WorkflowDefinition;
use serde::{Deserialize, Serialize};
pub const CONTROL_SCHEMA: u8 = 1;
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ControlRequest {
    AgentList,
    AgentKill {
        target: String,
    },
    AgentPause {
        target: String,
    },
    AgentResume {
        target: String,
    },
    AgentSpawn {
        provider: String,
        class: String,
        name: Option<String>,
        workspace: Option<String>,
    },
    AgentClone {
        target: String,
        name: Option<String>,
    },
    AgentWorktreeList,
    AgentWorktreeEnable {
        target: String,
        name: Option<String>,
    },
    AgentWorktreeJoin {
        target: String,
        worktree: String,
    },
    AgentWorktreeDisable {
        target: String,
    },
    WorkflowList,
    WorkflowShow {
        target: String,
    },
    WorkflowRun {
        id: String,
    },
    WorkflowStop {
        run_instance_id: String,
    },
    SendMessage {
        target: String,
        message: String,
        thread: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    AgentWatch {
        target: String,
        since: Option<String>,
        until: Option<String>,
        include: Vec<String>,
        tail_bytes: Option<usize>,
        follow: bool,
        timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageOrigin {
    WardianAgent { session_id: String },
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
pub struct DeliveryErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryDetail {
    pub uuid: String,
    pub name: String,
    pub provider: String,
    pub runtime_state: String,
    pub delivery_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DeliveryErrorDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendMessageResponse {
    pub schema: u8,
    pub ok: bool,
    pub delivery: Vec<DeliveryDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatchEvent {
    pub cursor: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchOutput {
    pub cursor: String,
    pub text: String,
    pub truncated: bool,
    pub omitted_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchDeliverySnapshot {
    pub delivery: Vec<DeliveryDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchAgentSnapshot {
    pub uuid: String,
    pub name: String,
    pub provider: String,
    pub status: String,
    pub last_status_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentWatchResponse {
    pub schema: u8,
    pub agent: WatchAgentSnapshot,
    pub cursor: String,
    pub events: Vec<WatchEvent>,
    pub output: WatchOutput,
    pub delivery: WatchDeliverySnapshot,
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
pub struct AgentWorktreeSummary {
    pub id: String,
    pub name: String,
    pub source_folder: String,
    pub worktree_folder: String,
    pub member_agent_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorktreeListResponse {
    pub schema: u8,
    pub worktrees: Vec<AgentWorktreeSummary>,
}

impl AgentWorktreeListResponse {
    pub fn new(worktrees: Vec<AgentWorktreeSummary>) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            worktrees,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorktreeMutationResponse {
    pub schema: u8,
    pub ok: bool,
    pub action: String,
    pub agent: AgentIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<AgentWorktreeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_worktree: Option<AgentWorktreeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    pub cleared_session: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResponse {
    pub schema: u8,
    pub workflow: WorkflowDefinition,
}

impl WorkflowResponse {
    pub fn new(workflow: WorkflowDefinition) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            workflow,
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
        let req = ControlRequest::AgentKill {
            target: "coder-a1".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""command":"agent_kill""#));
        assert!(json.contains(r#""target":"coder-a1""#));
    }

    #[test]
    fn agent_spawn_request_serializes_provider_and_class() {
        let req = ControlRequest::AgentSpawn {
            provider: "codex".to_string(),
            class: "Reviewer".to_string(),
            name: Some("CLI-Codex-Review".to_string()),
            workspace: Some("D:/Development/Wardian".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();

        assert!(json.contains(r#""command":"agent_spawn""#));
        assert!(json.contains(r#""provider":"codex""#));
        assert!(json.contains(r#""class":"Reviewer""#));
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
            origin: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""command":"send_message""#));
        assert!(json.contains(r#""target":"all""#));
        assert!(!json.contains(r#""origin""#));
    }

    #[test]
    fn send_message_request_serializes_agent_origin() {
        let req = ControlRequest::SendMessage {
            target: "CoderOne".to_string(),
            message: "hello".to_string(),
            thread: None,
            origin: Some(MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
        };

        let json = serde_json::to_string(&req).unwrap();

        assert!(json.contains(r#""origin""#));
        assert!(json.contains(r#""kind":"wardian_agent""#));
        assert!(json.contains(r#""session_id":"source-1""#));
    }

    #[test]
    fn send_message_request_accepts_missing_origin() {
        let json = r#"{"command":"send_message","target":"all","message":"hello","thread":null}"#;
        let req: ControlRequest = serde_json::from_str(json).unwrap();

        assert_eq!(
            req,
            ControlRequest::SendMessage {
                target: "all".to_string(),
                message: "hello".to_string(),
                thread: None,
                origin: None,
            }
        );
    }

    #[test]
    fn agent_watch_request_serializes_single_target_options() {
        let req = ControlRequest::AgentWatch {
            target: "Wardian-Codex".to_string(),
            since: Some("57244fa9-2b9c-4b45-ba32-6919d2786c29:0000000000000042".to_string()),
            until: Some("status:idle".to_string()),
            include: vec!["status".to_string(), "output".to_string()],
            tail_bytes: Some(4096),
            follow: false,
            timeout_ms: Some(30_000),
        };

        let json = serde_json::to_string(&req).unwrap();

        assert!(json.contains(r#""command":"agent_watch""#));
        assert!(json.contains(r#""target":"Wardian-Codex""#));
        assert!(json.contains(r#""until":"status:idle""#));
    }

    #[test]
    fn agent_worktree_requests_serialize() {
        let list = serde_json::to_string(&ControlRequest::AgentWorktreeList).unwrap();
        assert!(list.contains(r#""command":"agent_worktree_list""#));

        let enable = serde_json::to_string(&ControlRequest::AgentWorktreeEnable {
            target: "coder-a1".to_string(),
            name: Some("review fixes".to_string()),
        })
        .unwrap();
        assert!(enable.contains(r#""command":"agent_worktree_enable""#));
        assert!(enable.contains(r#""target":"coder-a1""#));
        assert!(enable.contains(r#""name":"review fixes""#));

        let join = serde_json::to_string(&ControlRequest::AgentWorktreeJoin {
            target: "coder-a1".to_string(),
            worktree: "D:/repo/worktrees/review".to_string(),
        })
        .unwrap();
        assert!(join.contains(r#""command":"agent_worktree_join""#));
        assert!(join.contains(r#""worktree":"D:/repo/worktrees/review""#));

        let disable = serde_json::to_string(&ControlRequest::AgentWorktreeDisable {
            target: "coder-a1".to_string(),
        })
        .unwrap();
        assert!(disable.contains(r#""command":"agent_worktree_disable""#));
    }

    #[test]
    fn agent_worktree_response_serializes_automation_fields() {
        let response = AgentWorktreeMutationResponse {
            schema: CONTROL_SCHEMA,
            ok: true,
            action: "enable".to_string(),
            agent: AgentResponse::new(AgentIdentity {
                name: "coder-a1".to_string(),
                uuid: "uuid-1".to_string(),
                class: "Coder".to_string(),
                provider: "codex".to_string(),
                status: "processing".to_string(),
                pid: None,
                started_at: None,
                workspace: Some("D:/repo/worktrees/review".to_string()),
                last_status_at: None,
                status_source: crate::identity::StatusSource::Live,
            })
            .agent,
            worktree: Some(AgentWorktreeSummary {
                id: "D:/repo/worktrees/review".to_string(),
                name: "review".to_string(),
                source_folder: "D:/repo".to_string(),
                worktree_folder: "D:/repo/worktrees/review".to_string(),
                member_agent_ids: vec!["uuid-1".to_string()],
            }),
            previous_worktree: None,
            previous_workspace: Some("D:/repo".to_string()),
            current_workspace: Some("D:/repo/worktrees/review".to_string()),
            branch_name: Some("wardian/review".to_string()),
            cleared_session: true,
        };

        let json = serde_json::to_string(&response).unwrap();

        assert!(json.contains(r#""schema":1"#));
        assert!(json.contains(r#""action":"enable""#));
        assert!(json.contains(r#""source_folder":"D:/repo""#));
        assert!(json.contains(r#""member_agent_ids":["uuid-1"]"#));
        assert!(json.contains(r#""cleared_session":true"#));
    }

    #[test]
    fn delivery_detail_splits_runtime_and_delivery_state() {
        let detail = DeliveryDetail {
            uuid: "agent-1".to_string(),
            name: "CoderOne".to_string(),
            provider: "codex".to_string(),
            runtime_state: "live_pty_available".to_string(),
            delivery_state: "submitted".to_string(),
            error: None,
        };

        let json = serde_json::to_string(&detail).unwrap();

        assert!(json.contains(r#""runtime_state":"live_pty_available""#));
        assert!(json.contains(r#""delivery_state":"submitted""#));
    }

    #[test]
    fn send_message_response_serializes_delivery_details() {
        let response = SendMessageResponse {
            schema: CONTROL_SCHEMA,
            ok: true,
            delivery: vec![DeliveryDetail {
                uuid: "agent-1".to_string(),
                name: "CoderOne".to_string(),
                provider: "codex".to_string(),
                runtime_state: "live_pty_available".to_string(),
                delivery_state: "submitted".to_string(),
                error: None,
            }],
        };

        let json = serde_json::to_string(&response).unwrap();

        assert!(json.contains(r#""ok":true"#));
        assert!(json.contains(r#""delivery_state":"submitted""#));
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

    #[test]
    fn workflow_show_request_serializes_with_target() {
        let req = ControlRequest::WorkflowShow {
            target: "wf-1".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""command":"workflow_show""#));
        assert!(json.contains(r#""target":"wf-1""#));
    }

    #[test]
    fn workflow_response_serializes_full_definition() {
        let workflow = crate::models::WorkflowDefinition {
            id: "wf-1".to_string(),
            name: "Daily Review".to_string(),
            settings: crate::models::WorkflowSettings {
                max_iterations: 10,
                on_limit_reached: "stop".to_string(),
            },
            nodes: vec![crate::models::WorkflowNode {
                id: "n1".to_string(),
                r#type: "agent".to_string(),
                name: Some("Coder".to_string()),
                config: serde_json::json!({"agent_class": "Coder"}),
                parameter_schema: None,
                dependencies: None,
                position: None,
            }],
            role_mappings: std::collections::HashMap::from([(
                "primary_coder".to_string(),
                "uuid-1".to_string(),
            )]),
        };

        let json = serde_json::to_string(&WorkflowResponse::new(workflow)).unwrap();

        assert!(json.contains(r#""workflow""#));
        assert!(json.contains(r#""nodes""#));
        assert!(json.contains(r#""role_mappings""#));
    }
}
