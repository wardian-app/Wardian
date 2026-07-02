use crate::conversations::{
    ConversationIndexEntry, ConversationManifest, ConversationNarrativeRecord,
};
use crate::identity::AgentIdentity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    ConversationList {
        agent: Option<String>,
        #[serde(default)]
        scope_all: bool,
    },
    ConversationShow {
        conversation_id: String,
    },
    SendMessage {
        target: String,
        message: String,
        thread: Option<String>,
        #[serde(default, skip_serializing_if = "MessageInputMode::is_message")]
        input_mode: MessageInputMode,
        #[serde(default, skip_serializing_if = "QueuePolicy::is_queue_if_busy")]
        queue_policy: QueuePolicy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_action: Option<ApprovalAction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        /// Target resolution scope: "neighbors" (default) or "all" — for agent senders only
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_scope: Option<String>,
    },
    Ask {
        target: String,
        message: String,
        thread: Option<String>,
        tail_bytes: Option<usize>,
        timeout_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    SubmitReply {
        request_id: String,
        status: ReplyStatus,
        body: String,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_echo_guard: Option<String>,
    },
    WorkflowRun {
        path: String,
        provider: Option<String>,
        workspace: Option<String>,
        input: Option<serde_json::Value>,
        bindings: Option<HashMap<String, String>>,
        assignments: Option<crate::models::WorkflowAssignments>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageOrigin {
    WardianAgent { session_id: String },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageInputMode {
    #[default]
    Message,
    Command,
    ApprovalAction,
}

impl MessageInputMode {
    pub fn is_message(&self) -> bool {
        matches!(self, Self::Message)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueuePolicy {
    #[default]
    QueueIfBusy,
    LiveOnly,
    MailboxOnly,
}

impl QueuePolicy {
    pub fn is_queue_if_busy(&self) -> bool {
        matches!(self, Self::QueueIfBusy)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ApprovalAction {
    Accept,
    Reject,
    Select { option: String },
    FreeText { text: String },
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
pub struct ConversationListResponse {
    pub schema: u8,
    pub conversations: Vec<ConversationIndexEntry>,
}

impl ConversationListResponse {
    pub fn new(conversations: Vec<ConversationIndexEntry>) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            conversations,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationShowResponse {
    pub schema: u8,
    pub manifest: ConversationManifest,
    pub conversation: Vec<ConversationNarrativeRecord>,
}

impl ConversationShowResponse {
    pub fn new(
        manifest: ConversationManifest,
        conversation: Vec<ConversationNarrativeRecord>,
    ) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            manifest,
            conversation,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunLaunchStatus {
    Started,
    ValidationFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowRunResponse {
    pub schema: u8,
    pub ok: bool,
    pub executor: String,
    pub status: WorkflowRunLaunchStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blueprint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<serde_json::Value>,
}

impl WorkflowRunResponse {
    pub fn started(
        executor: impl Into<String>,
        run_id: impl Into<String>,
        blueprint_id: impl Into<String>,
        run_dir: impl Into<String>,
    ) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            ok: true,
            executor: executor.into(),
            status: WorkflowRunLaunchStatus::Started,
            run_id: Some(run_id.into()),
            blueprint_id: Some(blueprint_id.into()),
            run_dir: Some(run_dir.into()),
            diagnostics: None,
        }
    }

    pub fn validation_failed(executor: impl Into<String>, diagnostics: serde_json::Value) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            ok: false,
            executor: executor.into(),
            status: WorkflowRunLaunchStatus::ValidationFailed,
            run_id: None,
            blueprint_id: None,
            run_dir: None,
            diagnostics: Some(diagnostics),
        }
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
    #[serde(default)]
    pub input_mode: MessageInputMode,
    #[serde(default, skip_serializing_if = "QueuePolicy::is_queue_if_busy")]
    pub queue_policy: QueuePolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DeliveryErrorDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendMessageResponse {
    pub schema: u8,
    pub ok: bool,
    pub delivery: Vec<DeliveryDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplyStatus {
    Done,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    Message,
    Task,
    Reply,
    Notification,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    Created,
    Queued,
    Delivering,
    Delivered,
    AwaitingReply,
    Completed,
    Failed,
    Expired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionTriggerPolicy {
    NotifyOnly,
    StartTurn,
    ReplyRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum InteractionBodyRef {
    Inline { body: String },
    File { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InteractionRecord {
    pub id: String,
    pub kind: InteractionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_session_id: Option<String>,
    pub target_session_ids: Vec<String>,
    pub status: InteractionStatus,
    pub trigger_policy: InteractionTriggerPolicy,
    pub body_ref: InteractionBodyRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_interaction_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryTransportKind {
    LiveSurface,
    HeadlessProcess,
    ProviderHook,
    ProviderPlugin,
    LocalControl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InteractionDeliveryAttemptRecord {
    pub id: String,
    pub interaction_id: String,
    pub target_session_id: String,
    pub transport: DeliveryTransportKind,
    pub generation: u64,
    pub runtime_state: String,
    pub delivery_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DeliveryErrorDetail>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderInputReadiness {
    Unknown,
    Booting,
    Ready,
    Busy,
    ActionRequired,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReadyEvidence {
    ProviderEvent,
    PromptDetected,
    TitleDetected,
    ManualStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderInputState {
    pub session_id: String,
    pub generation: u64,
    pub state: ProviderInputReadiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_evidence: Option<ProviderReadyEvidence>,
    pub observed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuredReply {
    pub request_id: String,
    pub status: ReplyStatus,
    pub body: String,
    pub target_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
    pub replied_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AskResponse {
    pub schema: u8,
    pub ok: bool,
    pub request_id: String,
    pub target: String,
    pub delivery: Vec<DeliveryDetail>,
    pub reply: StructuredReply,
    pub watch: AgentWatchResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch_error: Option<WatchEvidenceError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchEvidenceError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplyResponse {
    pub schema: u8,
    pub ok: bool,
    pub request_id: String,
    pub reply: StructuredReply,
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
pub struct WatchTranscriptMessage {
    pub role: String,
    pub text: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchTranscript {
    pub cursor: String,
    pub messages: Vec<WatchTranscriptMessage>,
    pub latest_text: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<WatchTranscript>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<WatchOutput>,
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
    #[serde(default)]
    pub can_delete: bool,
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
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: None,
            target_scope: None,
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
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: Some(MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
            target_scope: None,
        };

        let json = serde_json::to_string(&req).unwrap();

        assert!(json.contains(r#""origin""#));
        assert!(json.contains(r#""kind":"wardian_agent""#));
        assert!(json.contains(r#""session_id":"source-1""#));
    }

    #[test]
    fn send_message_request_serializes_command_input_mode() {
        let req = ControlRequest::SendMessage {
            target: "CoderOne".to_string(),
            message: "/goal test".to_string(),
            thread: None,
            input_mode: MessageInputMode::Command,
            queue_policy: QueuePolicy::QueueIfBusy,
            approval_action: None,
            origin: Some(MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
            target_scope: None,
        };

        let json = serde_json::to_string(&req).unwrap();

        assert!(json.contains(r#""input_mode":"command""#));
    }

    #[test]
    fn send_message_request_serializes_queue_policy_and_approval_action() {
        let req = ControlRequest::SendMessage {
            target: "CoderOne".to_string(),
            message: "approve".to_string(),
            thread: None,
            input_mode: MessageInputMode::ApprovalAction,
            queue_policy: QueuePolicy::LiveOnly,
            approval_action: Some(ApprovalAction::Select {
                option: "allow_once".to_string(),
            }),
            origin: None,
            target_scope: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let roundtrip: ControlRequest = serde_json::from_str(&json).unwrap();

        assert!(json.contains(r#""input_mode":"approval_action""#));
        assert!(json.contains(r#""queue_policy":"live_only""#));
        assert!(json.contains(r#""action":"select""#));
        assert!(json.contains(r#""option":"allow_once""#));
        assert_eq!(roundtrip, req);
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
                input_mode: MessageInputMode::Message,
                queue_policy: QueuePolicy::QueueIfBusy,
                approval_action: None,
                origin: None,
                target_scope: None,
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
            output_echo_guard: None,
        };

        let json = serde_json::to_string(&req).unwrap();

        assert!(json.contains(r#""command":"agent_watch""#));
        assert!(json.contains(r#""target":"Wardian-Codex""#));
        assert!(json.contains(r#""until":"status:idle""#));
        assert!(!json.contains(r#""output_echo_guard""#));
    }

    #[test]
    fn agent_watch_request_can_carry_internal_output_echo_guard() {
        let req = ControlRequest::AgentWatch {
            target: "Wardian-Codex".to_string(),
            since: Some("agent-1:0000000000000001".to_string()),
            until: Some("output:AUTO_TEST_2_DONE".to_string()),
            include: vec!["status".to_string(), "output".to_string()],
            tail_bytes: Some(4096),
            follow: false,
            timeout_ms: Some(30_000),
            output_echo_guard: Some("Say AUTO_TEST_2_DONE".to_string()),
        };

        let json = serde_json::to_string(&req).unwrap();
        let roundtrip: ControlRequest = serde_json::from_str(&json).unwrap();

        assert!(json.contains(r#""output_echo_guard":"Say AUTO_TEST_2_DONE""#));
        assert_eq!(roundtrip, req);
    }

    #[test]
    fn agent_watch_response_serializes_clean_output_without_raw_output_by_default() {
        let response = AgentWatchResponse {
            schema: CONTROL_SCHEMA,
            agent: WatchAgentSnapshot {
                uuid: "agent-1".to_string(),
                name: "CoderOne".to_string(),
                provider: "codex".to_string(),
                status: "idle".to_string(),
                last_status_at: None,
            },
            cursor: "agent-1:0000000000000002".to_string(),
            events: Vec::new(),
            output: WatchOutput {
                cursor: "agent-1:0000000000000002".to_string(),
                text: "clean answer".to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
            transcript: Some(WatchTranscript {
                cursor: "agent-1:0000000000000002".to_string(),
                messages: vec![WatchTranscriptMessage {
                    role: "assistant".to_string(),
                    text: "clean answer".to_string(),
                    provider: "codex".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    source: Some("response_item".to_string()),
                }],
                latest_text: "clean answer".to_string(),
                truncated: false,
                omitted_bytes: 0,
            }),
            raw_output: None,
            delivery: WatchDeliverySnapshot {
                delivery: Vec::new(),
            },
        };

        let json = serde_json::to_value(&response).unwrap();

        assert_eq!(json["output"]["text"], "clean answer");
        assert_eq!(json["transcript"]["latest_text"], "clean answer");
        assert!(json.get("raw_output").is_none());
    }

    #[test]
    fn agent_watch_response_serializes_raw_output_when_requested() {
        let response = AgentWatchResponse {
            schema: CONTROL_SCHEMA,
            agent: WatchAgentSnapshot {
                uuid: "agent-1".to_string(),
                name: "CoderOne".to_string(),
                provider: "codex".to_string(),
                status: "idle".to_string(),
                last_status_at: None,
            },
            cursor: "agent-1:0000000000000002".to_string(),
            events: Vec::new(),
            output: WatchOutput {
                cursor: "agent-1:0000000000000002".to_string(),
                text: "red".to_string(),
                truncated: false,
                omitted_bytes: 0,
            },
            transcript: None,
            raw_output: Some(WatchOutput {
                cursor: "agent-1:0000000000000002".to_string(),
                text: "\u{1b}[31mred\u{1b}[0m".to_string(),
                truncated: false,
                omitted_bytes: 0,
            }),
            delivery: WatchDeliverySnapshot {
                delivery: Vec::new(),
            },
        };

        let json = serde_json::to_value(&response).unwrap();

        assert_eq!(json["output"]["text"], "red");
        assert_eq!(json["raw_output"]["text"], "\u{1b}[31mred\u{1b}[0m");
    }

    #[test]
    fn ask_request_serializes_structured_reply_options() {
        let req = ControlRequest::Ask {
            target: "Wardian-Codex".to_string(),
            message: "review this".to_string(),
            thread: None,
            tail_bytes: Some(65_536),
            timeout_ms: Some(30_000),
            origin: Some(MessageOrigin::WardianAgent {
                session_id: "source-1".to_string(),
            }),
        };

        let json = serde_json::to_string(&req).unwrap();
        let roundtrip: ControlRequest = serde_json::from_str(&json).unwrap();

        assert!(json.contains(r#""command":"ask""#));
        assert!(json.contains(r#""target":"Wardian-Codex""#));
        assert!(json.contains(r#""message":"review this""#));
        assert!(json.contains(r#""tail_bytes":65536"#));
        assert!(json.contains(r#""timeout_ms":30000"#));
        assert_eq!(roundtrip, req);
    }

    #[test]
    fn ask_response_serializes_additive_watch_error() {
        let response = AskResponse {
            schema: CONTROL_SCHEMA,
            ok: true,
            request_id: "ask_0123456789abcdef".to_string(),
            target: "reviewer-a1".to_string(),
            delivery: Vec::new(),
            reply: StructuredReply {
                request_id: "ask_0123456789abcdef".to_string(),
                status: ReplyStatus::Done,
                body: "finished".to_string(),
                target_session_id: "agent-1".to_string(),
                source_session_id: Some("agent-1".to_string()),
                replied_at: "2026-05-22T00:00:00.000Z".to_string(),
            },
            watch: AgentWatchResponse {
                schema: CONTROL_SCHEMA,
                agent: WatchAgentSnapshot {
                    uuid: "agent-1".to_string(),
                    name: "reviewer-a1".to_string(),
                    provider: "codex".to_string(),
                    status: "idle".to_string(),
                    last_status_at: None,
                },
                cursor: "agent-1:0000000000000001".to_string(),
                events: Vec::new(),
                output: WatchOutput {
                    cursor: "agent-1:0000000000000001".to_string(),
                    text: String::new(),
                    truncated: false,
                    omitted_bytes: 0,
                },
                transcript: None,
                raw_output: None,
                delivery: WatchDeliverySnapshot {
                    delivery: Vec::new(),
                },
            },
            watch_error: Some(WatchEvidenceError {
                code: "cursor_expired".to_string(),
                message: "watch state error".to_string(),
            }),
        };

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: AskResponse = serde_json::from_str(&json).unwrap();

        assert!(json.contains(r#""watch_error""#));
        assert_eq!(
            roundtrip
                .watch_error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("cursor_expired")
        );
        assert_eq!(roundtrip.reply.body, "finished");
    }

    #[test]
    fn submit_reply_request_serializes_status_and_body() {
        let req = ControlRequest::SubmitReply {
            request_id: "ask_0123456789abcdef".to_string(),
            status: ReplyStatus::Done,
            body: "finished".to_string(),
            origin: Some(MessageOrigin::WardianAgent {
                session_id: "agent-1".to_string(),
            }),
        };

        let json = serde_json::to_string(&req).unwrap();
        let roundtrip: ControlRequest = serde_json::from_str(&json).unwrap();

        assert!(json.contains(r#""command":"submit_reply""#));
        assert!(json.contains(r#""request_id":"ask_0123456789abcdef""#));
        assert!(json.contains(r#""status":"done""#));
        assert!(json.contains(r#""body":"finished""#));
        assert_eq!(roundtrip, req);
    }

    #[test]
    fn interaction_record_serializes_stable_fields() {
        let record = InteractionRecord {
            id: "int_001".to_string(),
            kind: InteractionKind::Task,
            sender_session_id: Some("source-1".to_string()),
            target_session_ids: vec!["agent-1".to_string()],
            status: InteractionStatus::AwaitingReply,
            trigger_policy: InteractionTriggerPolicy::ReplyRequired,
            body_ref: InteractionBodyRef::Inline {
                body: "review this".to_string(),
            },
            parent_interaction_id: None,
            created_at: "2026-05-25T00:00:00.000Z".to_string(),
            updated_at: "2026-05-25T00:00:01.000Z".to_string(),
            completed_at: None,
        };

        let json = serde_json::to_string(&record).unwrap();

        assert!(json.contains(r#""id":"int_001""#));
        assert!(json.contains(r#""kind":"task""#));
        assert!(json.contains(r#""status":"awaiting_reply""#));
        assert!(json.contains(r#""trigger_policy":"reply_required""#));
    }

    #[test]
    fn provider_input_state_serializes_generation_and_evidence() {
        let state = ProviderInputState {
            session_id: "agent-1".to_string(),
            generation: 7,
            state: ProviderInputReadiness::Ready,
            ready_evidence: Some(ProviderReadyEvidence::PromptDetected),
            observed_at: "2026-05-25T00:00:00.000Z".to_string(),
        };

        let json = serde_json::to_string(&state).unwrap();

        assert!(json.contains(r#""generation":7"#));
        assert!(json.contains(r#""state":"ready""#));
        assert!(json.contains(r#""ready_evidence":"prompt_detected""#));
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
    fn conversation_requests_serialize() {
        let list = serde_json::to_string(&ControlRequest::ConversationList {
            agent: Some("agent-1".to_string()),
            scope_all: true,
        })
        .unwrap();
        assert!(list.contains(r#""command":"conversation_list""#));
        assert!(list.contains(r#""agent":"agent-1""#));
        assert!(list.contains(r#""scope_all":true"#));

        let default_list: ControlRequest =
            serde_json::from_str(r#"{"command":"conversation_list"}"#).unwrap();
        assert_eq!(
            default_list,
            ControlRequest::ConversationList {
                agent: None,
                scope_all: false,
            }
        );

        let show = serde_json::to_string(&ControlRequest::ConversationShow {
            conversation_id: "conv_20260615_agent_1".to_string(),
        })
        .unwrap();
        let roundtrip: ControlRequest = serde_json::from_str(&show).unwrap();

        assert!(show.contains(r#""command":"conversation_show""#));
        assert!(show.contains(r#""conversation_id":"conv_20260615_agent_1""#));
        assert_eq!(
            roundtrip,
            ControlRequest::ConversationShow {
                conversation_id: "conv_20260615_agent_1".to_string(),
            }
        );
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
                visibility: None,
            })
            .agent,
            worktree: Some(AgentWorktreeSummary {
                id: "D:/repo/worktrees/review".to_string(),
                name: "review".to_string(),
                source_folder: "D:/repo".to_string(),
                worktree_folder: "D:/repo/worktrees/review".to_string(),
                member_agent_ids: vec!["uuid-1".to_string()],
                can_delete: false,
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
        assert!(json.contains(r#""can_delete":false"#));
        assert!(json.contains(r#""cleared_session":true"#));
    }

    #[test]
    fn workflow_run_request_serializes_live_launch_options() {
        let req = ControlRequest::WorkflowRun {
            path: "<absolute-workspace-path>/library/workflows/autoreview.md".to_string(),
            provider: Some("codex".to_string()),
            workspace: Some("<absolute-workspace-path>".to_string()),
            input: Some(serde_json::json!({ "target": "HEAD" })),
            bindings: Some(std::collections::HashMap::from([(
                "reviewer".to_string(),
                "codex".to_string(),
            )])),
            assignments: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let roundtrip: ControlRequest = serde_json::from_str(&json).unwrap();

        assert!(json.contains(r#""command":"workflow_run""#));
        assert!(
            json.contains(r#""path":"<absolute-workspace-path>/library/workflows/autoreview.md""#)
        );
        assert!(json.contains(r#""provider":"codex""#));
        assert!(json.contains(r#""bindings":{"reviewer":"codex"}"#));
        assert_eq!(roundtrip, req);
    }

    #[test]
    fn workflow_run_response_serializes_live_start_contract() {
        let response = WorkflowRunResponse::started(
            "live",
            "run-1",
            "autoreview",
            "<absolute-workspace-path>/logs/workflows/autoreview/run-1",
        );

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: WorkflowRunResponse = serde_json::from_str(&json).unwrap();

        assert!(json.contains(r#""schema":1"#));
        assert!(json.contains(r#""executor":"live""#));
        assert!(json.contains(r#""status":"started""#));
        assert_eq!(roundtrip, response);
    }

    #[test]
    fn delivery_detail_splits_runtime_and_delivery_state() {
        let detail = DeliveryDetail {
            uuid: "agent-1".to_string(),
            name: "CoderOne".to_string(),
            provider: "codex".to_string(),
            runtime_state: "live_pty_available".to_string(),
            delivery_state: "submitted".to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            message_id: None,
            delivery_phase: None,
            observed_state: None,
            reason: None,
            profile: None,
            error: None,
        };

        let json = serde_json::to_string(&detail).unwrap();

        assert!(json.contains(r#""runtime_state":"live_pty_available""#));
        assert!(json.contains(r#""delivery_state":"submitted""#));
    }

    #[test]
    fn delivery_detail_serializes_rich_delivery_fields() {
        let detail = DeliveryDetail {
            uuid: "agent-1".to_string(),
            name: "CoderOne".to_string(),
            provider: "codex".to_string(),
            runtime_state: "live_pty_available".to_string(),
            delivery_state: "submitted".to_string(),
            input_mode: MessageInputMode::ApprovalAction,
            queue_policy: QueuePolicy::MailboxOnly,
            message_id: Some("msg_1".to_string()),
            delivery_phase: Some("submit".to_string()),
            observed_state: Some("submitted_observed".to_string()),
            reason: Some("target_processing".to_string()),
            profile: Some("codex".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&detail).unwrap();

        assert!(json.contains(r#""queue_policy":"mailbox_only""#));
        assert!(json.contains(r#""message_id":"msg_1""#));
        assert!(json.contains(r#""delivery_phase":"submit""#));
        assert!(json.contains(r#""observed_state":"submitted_observed""#));
        assert!(json.contains(r#""reason":"target_processing""#));
        assert!(json.contains(r#""profile":"codex""#));
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
                input_mode: MessageInputMode::Command,
                queue_policy: QueuePolicy::QueueIfBusy,
                message_id: None,
                delivery_phase: None,
                observed_state: None,
                reason: None,
                profile: None,
                error: None,
            }],
        };

        let json = serde_json::to_string(&response).unwrap();

        assert!(json.contains(r#""ok":true"#));
        assert!(json.contains(r#""delivery_state":"submitted""#));
        assert!(json.contains(r#""input_mode":"command""#));
    }

    #[test]
    fn delivery_attempt_record_serializes_transport_kind() {
        let record = InteractionDeliveryAttemptRecord {
            id: "attempt_1".to_string(),
            interaction_id: "int_1".to_string(),
            target_session_id: "agent-1".to_string(),
            transport: DeliveryTransportKind::LiveSurface,
            generation: 7,
            runtime_state: "live_pty_available".to_string(),
            delivery_state: "submit_sent_unconfirmed".to_string(),
            delivery_phase: Some("submit_key_sent".to_string()),
            observed_state: Some("bytes_sent".to_string()),
            reason: None,
            error: None,
            created_at: "2026-06-07T00:00:00.000Z".to_string(),
            updated_at: "2026-06-07T00:00:00.000Z".to_string(),
        };

        let json = serde_json::to_string(&record).expect("serialize attempt");

        assert!(json.contains(r#""transport":"live_surface""#));
        assert!(json.contains(r#""delivery_state":"submit_sent_unconfirmed""#));
    }

    #[test]
    fn delivery_transport_kind_deserializes_headless_process() {
        let value: DeliveryTransportKind =
            serde_json::from_str(r#""headless_process""#).expect("deserialize transport kind");

        assert_eq!(value, DeliveryTransportKind::HeadlessProcess);
    }
}
