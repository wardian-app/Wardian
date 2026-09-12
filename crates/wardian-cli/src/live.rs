pub(crate) mod messaging;

use std::{
    fmt, io,
    time::{Duration, Instant},
};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::browser::{
    BrowserActionResult, BrowserGetResult, BrowserScreenshotResult, BrowserSessionSummary,
    ConsoleEntry, PageSnapshot,
};
use wardian_core::control::{
    AgentDoctorResponse, AgentListResponse, AgentResponse, AgentUpdateResponse, AgentWatchResponse,
    AgentWorktreeListResponse, AgentWorktreeMutationResponse, AgentWorktreeSummary, ApprovalAction,
    AskManyResponse, AskResponse, AutomationRunResponse, ControlRequest, ConversationListResponse,
    ConversationShowResponse, DeliveryDetail, InboxListResponse, InboxNotificationPayload,
    InboxNotificationResponse, MessageInputMode, MessageOrigin, NativeDeliveryCapabilitiesResponse,
    NativeDeliveryInspectResponse, OrchestrationDeliveryOptions, QueuePolicy, ReplyResponse,
    ReplyStatus, SendMessageResponse, StructuredReply, WatchEvent, WatchEvidenceError,
};
use wardian_core::identity::AgentIdentity;
use wardian_core::native_transport::NativeDeliveryPhase;

const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);
const CONTROL_GIT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(15);
const CONTROL_MUTATION_TIMEOUT: Duration = Duration::from_secs(30);

struct AgentWatchRequest<'a> {
    target: &'a str,
    since: Option<&'a str>,
    until: Option<&'a str>,
    include: Vec<String>,
    tail_bytes: Option<usize>,
    follow: bool,
    timeout: Duration,
    output_echo_guard: Option<&'a str>,
}

struct SendAndWatchRequest<'a> {
    target: &'a str,
    message: &'a str,
    thread: Option<&'a str>,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    approval_action: Option<ApprovalAction>,
    condition: &'a str,
    tail_bytes: Option<usize>,
    timeout: Duration,
    output_echo_guard: Option<&'a str>,
    target_scope: Option<&'a str>,
    orchestration: Option<OrchestrationDeliveryOptions>,
}

pub struct SendMessageAndWatchOptions<'a> {
    pub thread: Option<&'a str>,
    pub input_mode: MessageInputMode,
    pub queue_policy: QueuePolicy,
    pub approval_action: Option<ApprovalAction>,
    pub until: &'a str,
    pub timeout: Duration,
    pub target_scope: Option<&'a str>,
    pub orchestration: Option<OrchestrationDeliveryOptions>,
}

pub struct SendMessageAndWatchConditionOptions<'a> {
    pub thread: Option<&'a str>,
    pub input_mode: MessageInputMode,
    pub queue_policy: QueuePolicy,
    pub approval_action: Option<ApprovalAction>,
    pub condition: &'a str,
    pub tail_bytes: Option<usize>,
    pub timeout: Duration,
    pub target_scope: Option<&'a str>,
    pub orchestration: Option<OrchestrationDeliveryOptions>,
}

pub struct SendMessageDeliveryOptions<'a> {
    pub thread: Option<&'a str>,
    pub input_mode: MessageInputMode,
    pub queue_policy: QueuePolicy,
    pub approval_action: Option<ApprovalAction>,
    pub target_scope: Option<&'a str>,
    pub timeout: Duration,
    pub orchestration: Option<OrchestrationDeliveryOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlOperation {
    AgentList,
    AgentDoctor,
    AgentDelete,
    AgentRename,
    AgentRestart,
    AgentPause,
    AgentResume,
    AgentModels,
    AgentSpawn,
    AgentUpdate,
    AgentClone,
    AgentWorktreeList,
    AgentWorktreeEnable,
    AgentWorktreeJoin,
    AgentWorktreeDisable,
    ConversationList,
    ConversationShow,
    InboxList,
    ArtifactPresent,
    ArtifactShow,
    ArtifactReviewShow,
    WatchlistsChanged,
    TopologyMutate,
    AutomationRun,
    SendMessage {
        requested: Duration,
    },
    NotifyCreate,
    NotifyWait {
        requested: Duration,
    },
    Ask {
        requested: Duration,
        target: String,
    },
    SubmitReply,
    AgentWatch {
        requested: Duration,
        target: String,
        until: String,
    },
    /// Any `wardian browser` call. The budget is the call's own, because a
    /// `wait` is bounded by its `--timeout-ms` while an `open` is bounded by
    /// how long a browser takes to start.
    Browser {
        requested: Duration,
    },
}

#[derive(Debug)]
pub struct ControlEndpointError {
    code: String,
    message: String,
    details: Option<serde_json::Value>,
}

impl ControlEndpointError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(
        code: impl Into<String>,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Some(details),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn details(&self) -> Option<&serde_json::Value> {
        self.details.as_ref()
    }
}

impl fmt::Display for ControlEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ControlEndpointError {}

#[derive(Debug)]
pub struct WaitTimeoutError {
    target: String,
    until: String,
    last_status: String,
}

impl WaitTimeoutError {
    pub fn new(target: &str, until: &str, last_status: &str) -> Self {
        Self {
            target: target.to_string(),
            until: until.to_string(),
            last_status: last_status.to_string(),
        }
    }
}

impl fmt::Display for WaitTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "timed out waiting for {} to reach {}; last status: {}",
            self.target, self.until, self.last_status
        )
    }
}

impl std::error::Error for WaitTimeoutError {}

#[derive(Debug)]
pub struct WatchTimeoutError {
    target: String,
    until: String,
    last_status: String,
}

impl WatchTimeoutError {
    pub fn new(target: &str, until: &str, last_status: &str) -> Self {
        Self {
            target: target.to_string(),
            until: until.to_string(),
            last_status: last_status.to_string(),
        }
    }
}

impl fmt::Display for WatchTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "timed out watching {} for {}; last status: {}",
            self.target, self.until, self.last_status
        )
    }
}

impl std::error::Error for WatchTimeoutError {}

#[derive(Debug)]
pub struct WaitTargetNotFoundError {
    target: String,
}

impl WaitTargetNotFoundError {
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_string(),
        }
    }
}

impl fmt::Display for WaitTargetNotFoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent not found: {}", self.target)
    }
}

impl std::error::Error for WaitTargetNotFoundError {}

pub struct AskAgentResponse {
    pub request_id: Option<String>,
    pub reply: Option<StructuredReply>,
    pub delivery: Vec<DeliveryDetail>,
    pub watch_error: Option<WatchEvidenceError>,
    pub watch: AgentWatchResponse,
}

pub struct AutomationRunRequest {
    pub path: String,
    pub provider: Option<String>,
    pub workspace: Option<String>,
    pub input: serde_json::Value,
    pub bindings: std::collections::HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn list_agents() -> io::Result<Vec<AgentIdentity>> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentList,
        send_request(ControlRequest::AgentList),
    )?;
    let response: AgentListResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(response.agents)
}

/// Topology is the control plane's single writer: every mutation is
/// authorized and persisted by the app, never by the CLI directly.
pub fn topology_link(
    a: &str,
    b: &str,
    caller_session_id: Option<&str>,
) -> io::Result<wardian_core::control::TopologyMutationResponse> {
    topology_mutate(ControlRequest::TopologyLink {
        a: a.to_string(),
        b: b.to_string(),
        caller_session_id: caller_session_id.map(str::to_string),
    })
}

pub fn topology_unlink(
    a: &str,
    b: &str,
    caller_session_id: Option<&str>,
) -> io::Result<wardian_core::control::TopologyMutationResponse> {
    topology_mutate(ControlRequest::TopologyUnlink {
        a: a.to_string(),
        b: b.to_string(),
        caller_session_id: caller_session_id.map(str::to_string),
    })
}

pub fn topology_ignore(
    a: &str,
    b: &str,
    caller_session_id: Option<&str>,
) -> io::Result<wardian_core::control::TopologyMutationResponse> {
    topology_mutate(ControlRequest::TopologyIgnore {
        a: a.to_string(),
        b: b.to_string(),
        caller_session_id: caller_session_id.map(str::to_string),
    })
}

pub fn topology_unignore(
    a: &str,
    b: &str,
    caller_session_id: Option<&str>,
) -> io::Result<wardian_core::control::TopologyMutationResponse> {
    topology_mutate(ControlRequest::TopologyUnignore {
        a: a.to_string(),
        b: b.to_string(),
        caller_session_id: caller_session_id.map(str::to_string),
    })
}

fn topology_mutate(
    request: ControlRequest,
) -> io::Result<wardian_core::control::TopologyMutationResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::TopologyMutate,
        send_request(request),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn agent_doctor(target: &str) -> io::Result<AgentDoctorResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentDoctor,
        send_request(ControlRequest::AgentDoctor {
            target: target.to_string(),
        }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
}

pub fn agent_delete(target: &str, confirm_name: &str, force: bool) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::AgentDelete,
        send_request(ControlRequest::AgentDelete {
            target: target.to_string(),
            confirm_name: confirm_name.to_string(),
            force,
        }),
    )
    .map(|_| ())
}

pub fn agent_rename(target: &str, name: &str) -> io::Result<AgentUpdateResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentRename,
        send_request(ControlRequest::AgentRename {
            target: target.to_string(),
            name: name.to_string(),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn agent_restart(target: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::AgentRestart,
        send_request(ControlRequest::AgentRestart {
            target: target.to_string(),
        }),
    )
    .map(|_| ())
}

pub fn agent_pause(target: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::AgentPause,
        send_request(ControlRequest::AgentPause {
            target: target.to_string(),
        }),
    )
    .map(|_| ())
}

pub fn agent_resume(target: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::AgentResume,
        send_request(ControlRequest::AgentResume {
            target: target.to_string(),
        }),
    )
    .map(|_| ())
}

pub fn agent_spawn(
    provider: &str,
    class: &str,
    name: Option<&str>,
    workspace: Option<&str>,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
) -> io::Result<AgentIdentity> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentSpawn,
        send_request(ControlRequest::AgentSpawn {
            provider: provider.to_string(),
            class: class.to_string(),
            name: name.map(str::to_string),
            workspace: workspace.map(str::to_string),
            model: model.map(str::to_string),
            reasoning_effort: reasoning_effort.map(str::to_string),
        }),
    )?;
    let resp: AgentResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(resp.agent)
}

pub fn agent_models(provider: &str, force_refresh: bool) -> io::Result<serde_json::Value> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::AgentModels,
        send_request(ControlRequest::AgentModels {
            provider: provider.to_string(),
            force_refresh,
        }),
    )
}

pub fn agent_update(
    target: &str,
    class: Option<&str>,
    workspace: Option<&str>,
    description: Option<&str>,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
) -> io::Result<AgentUpdateResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentUpdate,
        send_request(ControlRequest::AgentUpdate {
            target: target.to_string(),
            class: class.map(str::to_string),
            workspace: workspace.map(str::to_string),
            description: description.map(str::to_string),
            model: model.map(str::to_string),
            reasoning_effort: reasoning_effort.map(str::to_string),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn agent_clone(target: &str, name: Option<&str>) -> io::Result<AgentIdentity> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentClone,
        send_request(ControlRequest::AgentClone {
            target: target.to_string(),
            name: name.map(str::to_string),
        }),
    )?;
    let resp: AgentResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(resp.agent)
}

pub fn agent_worktree_list() -> io::Result<Vec<AgentWorktreeSummary>> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentWorktreeList,
        send_request(ControlRequest::AgentWorktreeList),
    )?;
    let resp: AgentWorktreeListResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(resp.worktrees)
}

pub fn agent_worktree_enable(
    target: &str,
    name: Option<&str>,
) -> io::Result<AgentWorktreeMutationResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentWorktreeEnable,
        send_request(ControlRequest::AgentWorktreeEnable {
            target: target.to_string(),
            name: name.map(str::to_string),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn agent_worktree_join(
    target: &str,
    worktree: &str,
) -> io::Result<AgentWorktreeMutationResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentWorktreeJoin,
        send_request(ControlRequest::AgentWorktreeJoin {
            target: target.to_string(),
            worktree: worktree.to_string(),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn agent_worktree_disable(target: &str) -> io::Result<AgentWorktreeMutationResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentWorktreeDisable,
        send_request(ControlRequest::AgentWorktreeDisable {
            target: target.to_string(),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn conversation_list(
    agent: Option<&str>,
    scope_all: bool,
) -> io::Result<ConversationListResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::ConversationList,
        send_request(ControlRequest::ConversationList {
            agent: agent.map(str::to_string),
            scope_all,
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn conversation_show(conversation_id: &str) -> io::Result<ConversationShowResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::ConversationShow,
        send_request(ControlRequest::ConversationShow {
            conversation_id: conversation_id.to_string(),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn inbox_list_page(
    offset: usize,
    types: Vec<String>,
    sources: Vec<String>,
    unread: bool,
    limit: usize,
) -> io::Result<InboxListResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::InboxList,
        send_request(ControlRequest::InboxList {
            offset,
            types,
            sources,
            unread,
            limit,
        }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub fn artifact_present(
    path: &str,
    title: Option<&str>,
    description: Option<&str>,
    artifact_id: Option<&str>,
    force_new: bool,
    addressed_comment_ids: &[String],
    origin: MessageOrigin,
) -> io::Result<serde_json::Value> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::ArtifactPresent,
        send_request(ControlRequest::ArtifactPresent {
            path: path.to_string(),
            title: title.map(str::to_string),
            description: description.map(str::to_string),
            artifact_id: artifact_id.map(str::to_string),
            force_new,
            addressed_comment_ids: addressed_comment_ids.to_vec(),
            origin,
        }),
    )
}

pub fn artifact_show(artifact_id: &str, version_id: Option<&str>) -> io::Result<serde_json::Value> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::ArtifactShow,
        send_request(ControlRequest::ArtifactShow {
            artifact_id: artifact_id.to_string(),
            version_id: version_id.map(str::to_string),
        }),
    )
}

pub fn artifact_review_show(
    artifact_id: &str,
    review_id: Option<&str>,
    latest: bool,
) -> io::Result<serde_json::Value> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::ArtifactReviewShow,
        send_request(ControlRequest::ArtifactReviewShow {
            artifact_id: artifact_id.to_string(),
            review_id: review_id.map(str::to_string),
            latest,
        }),
    )
}

pub fn notify_watchlists_changed() -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::WatchlistsChanged,
        send_request(ControlRequest::WatchlistsChanged),
    )
    .map(|_| ())
}

pub fn automation_run(request: AutomationRunRequest) -> io::Result<AutomationRunResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AutomationRun,
        send_request(ControlRequest::AutomationRun {
            path: request.path,
            provider: request.provider,
            workspace: request.workspace,
            input: Some(request.input),
            bindings: Some(request.bindings),
            assignments: None,
            caller_agent_id: std::env::var("WARDIAN_SESSION_ID").ok(),
            memory_capability: std::env::var(wardian_core::memory::MEMORY_CAPABILITY_ENV).ok(),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn send_message_with_delivery_and_scope_options(
    target: &str,
    message: &str,
    options: SendMessageDeliveryOptions<'_>,
) -> io::Result<SendMessageResponse> {
    let SendMessageDeliveryOptions {
        thread,
        input_mode,
        queue_policy,
        approval_action,
        target_scope,
        timeout,
        orchestration,
    } = options;
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::SendMessage { requested: timeout },
        send_request(ControlRequest::SendMessage {
            target: target.to_string(),
            message: message.to_string(),
            thread: thread.map(str::to_string),
            input_mode,
            queue_policy,
            approval_action,
            origin: current_message_origin(),
            target_scope: target_scope.map(str::to_string),
            headless_timeout_ms: Some(timeout.as_millis().try_into().unwrap_or(u64::MAX)),
            orchestration,
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn require_current_message_origin() -> io::Result<MessageOrigin> {
    current_message_origin().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "WARDIAN_SESSION_ID environment variable is not set",
        )
    })
}

pub fn create_notification(
    notification: InboxNotificationPayload,
    origin: MessageOrigin,
) -> io::Result<InboxNotificationResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::NotifyCreate,
        send_request(ControlRequest::NotifyCreate {
            notification,
            origin,
        }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
}

pub fn wait_for_notification(
    notification_id: &str,
    timeout: Duration,
) -> io::Result<InboxNotificationResponse> {
    let origin = require_current_message_origin()?;
    let timeout_ms = u64::try_from(timeout.as_millis()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "notification timeout is too large",
        )
    })?;
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::NotifyWait { requested: timeout },
        send_request(ControlRequest::NotifyWait {
            notification_id: notification_id.to_string(),
            timeout_ms: Some(timeout_ms),
            origin,
        }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
}

pub fn submit_reply(
    request_id: &str,
    status: ReplyStatus,
    body: &str,
) -> io::Result<ReplyResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::SubmitReply,
        send_request(ControlRequest::SubmitReply {
            request_id: request_id.to_string(),
            status,
            body: body.to_string(),
            origin: current_message_origin(),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

pub fn delivery_get(
    interaction_id: &str,
    evidence_limit: usize,
) -> io::Result<NativeDeliveryInspectResponse> {
    send_delivery_request(ControlRequest::DeliveryGet {
        interaction_id: interaction_id.to_string(),
        evidence_limit: Some(evidence_limit),
    })
}

pub fn delivery_cancel(interaction_id: &str) -> io::Result<NativeDeliveryInspectResponse> {
    send_delivery_request(ControlRequest::DeliveryCancel {
        interaction_id: interaction_id.to_string(),
    })
}

pub fn delivery_withdraw(interaction_id: &str) -> io::Result<NativeDeliveryInspectResponse> {
    send_delivery_request(ControlRequest::DeliveryWithdraw {
        interaction_id: interaction_id.to_string(),
    })
}

pub fn delivery_replace(
    interaction_id: &str,
    message: &str,
    idempotency_key: &str,
    deadline_at: Option<String>,
) -> io::Result<NativeDeliveryInspectResponse> {
    send_delivery_request(ControlRequest::DeliveryReplace {
        interaction_id: interaction_id.to_string(),
        message: message.to_string(),
        idempotency_key: idempotency_key.to_string(),
        deadline_at,
    })
}

pub fn delivery_capabilities(target: &str) -> io::Result<NativeDeliveryCapabilitiesResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::SendMessage {
            requested: CONTROL_MUTATION_TIMEOUT,
        },
        send_request(ControlRequest::DeliveryCapabilities {
            target: target.to_string(),
        }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
}

fn send_delivery_request(request: ControlRequest) -> io::Result<NativeDeliveryInspectResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::SendMessage {
            requested: CONTROL_MUTATION_TIMEOUT,
        },
        send_request(request),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
}

/// Wait for a matching status within one elapsed-time budget, including IPC and
/// polling sleeps. A zero budget expires without reading the endpoint.
pub fn wait_agent_until(target: &str, until: &str, timeout: Duration) -> io::Result<AgentIdentity> {
    wait_agent_until_after_snapshot(target, until, timeout, None)
}

pub fn agent_watch(
    target: &str,
    since: Option<&str>,
    until: Option<&str>,
    include: Vec<String>,
    tail_bytes: Option<usize>,
    follow: bool,
    timeout: Duration,
) -> io::Result<AgentWatchResponse> {
    agent_watch_with_output_echo_guard(AgentWatchRequest {
        target,
        since,
        until,
        include,
        tail_bytes,
        follow,
        timeout,
        output_echo_guard: None,
    })
}

fn agent_watch_with_output_echo_guard(
    request: AgentWatchRequest<'_>,
) -> io::Result<AgentWatchResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::AgentWatch {
            requested: request.timeout,
            target: request.target.to_string(),
            until: request.until.unwrap_or("snapshot").to_string(),
        },
        send_request(ControlRequest::AgentWatch {
            target: request.target.to_string(),
            since: request.since.map(str::to_string),
            until: request.until.map(str::to_string),
            include: request.include,
            tail_bytes: request.tail_bytes,
            follow: request.follow,
            timeout_ms: Some(request.timeout.as_millis().try_into().unwrap_or(u64::MAX)),
            output_echo_guard: request.output_echo_guard.map(str::to_string),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
}

/// Wait for a newer matching observation, charging the initial cursor snapshot
/// and subsequent watch against the same elapsed-time budget.
pub fn wait_agent_until_next(
    target: &str,
    until: &str,
    timeout: Duration,
) -> io::Result<AgentWatchResponse> {
    wait_agent_until_next_with(target, until, timeout, |since, condition, budget| {
        let runtime = build_runtime()?;
        let remaining = budget.remaining().ok_or_else(wait_transport_timeout)?;
        let requested = if since.is_none() {
            remaining.min(Duration::from_secs(5))
        } else {
            remaining
        };
        let value = wait_transport(
            &runtime,
            budget,
            watch_timeout_for(requested),
            send_request(ControlRequest::AgentWatch {
                target: target.to_string(),
                since: since.map(str::to_string),
                until: condition.map(str::to_string),
                include: vec!["status".to_string()],
                tail_bytes: Some(4096),
                follow: false,
                timeout_ms: Some(requested.as_millis().try_into().unwrap_or(u64::MAX)),
                output_echo_guard: None,
            }),
        )?;
        serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
    })
}

pub fn send_message_and_watch(
    target: &str,
    message: &str,
    options: SendMessageAndWatchOptions<'_>,
) -> io::Result<AskAgentResponse> {
    send_message_and_watch_condition(
        target,
        message,
        SendMessageAndWatchConditionOptions {
            thread: options.thread,
            input_mode: options.input_mode,
            queue_policy: options.queue_policy,
            approval_action: options.approval_action,
            condition: &format!("status:{}", options.until),
            tail_bytes: Some(4096),
            timeout: options.timeout,
            target_scope: options.target_scope,
            orchestration: options.orchestration,
        },
    )
}

pub fn ask_agent(
    target: &str,
    message: &str,
    thread: Option<&str>,
    condition: &str,
    tail_bytes: Option<usize>,
    timeout: Duration,
    orchestration: Option<OrchestrationDeliveryOptions>,
) -> io::Result<AskAgentResponse> {
    if condition == "reply" {
        return ask_agent_structured(target, message, thread, tail_bytes, timeout, orchestration);
    }
    send_message_and_watch_condition_with_output_echo_guard(SendAndWatchRequest {
        target,
        message,
        thread,
        input_mode: MessageInputMode::Message,
        queue_policy: QueuePolicy::QueueIfBusy,
        approval_action: None,
        condition,
        tail_bytes,
        timeout,
        output_echo_guard: ask_prompt_echo_guard(condition, message),
        target_scope: None,
        orchestration,
    })
}

pub fn ask_agents(
    targets: &[String],
    message: &str,
    thread: Option<&str>,
    tail_bytes: Option<usize>,
    timeout: Duration,
    orchestration: Option<OrchestrationDeliveryOptions>,
) -> io::Result<AskManyResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::Ask {
            requested: timeout,
            target: targets.join(","),
        },
        send_request(ControlRequest::AskMany {
            targets: targets.to_vec(),
            message: message.to_string(),
            thread: thread.map(str::to_string),
            tail_bytes,
            timeout_ms: Some(timeout.as_millis().try_into().unwrap_or(u64::MAX)),
            origin: current_message_origin(),
            orchestration,
        }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
}

fn ask_agent_structured(
    target: &str,
    message: &str,
    thread: Option<&str>,
    tail_bytes: Option<usize>,
    timeout: Duration,
    orchestration: Option<OrchestrationDeliveryOptions>,
) -> io::Result<AskAgentResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::Ask {
            requested: timeout,
            target: target.to_string(),
        },
        send_request(ControlRequest::Ask {
            target: target.to_string(),
            message: message.to_string(),
            thread: thread.map(str::to_string),
            tail_bytes,
            timeout_ms: Some(timeout.as_millis().try_into().unwrap_or(u64::MAX)),
            origin: current_message_origin(),
            orchestration,
        }),
    )?;
    let response: AskResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(AskAgentResponse {
        request_id: Some(response.request_id),
        reply: Some(response.reply),
        delivery: response.delivery,
        watch_error: response.watch_error,
        watch: response.watch,
    })
}

fn send_message_and_watch_condition(
    target: &str,
    message: &str,
    options: SendMessageAndWatchConditionOptions<'_>,
) -> io::Result<AskAgentResponse> {
    send_message_and_watch_condition_with_output_echo_guard(SendAndWatchRequest {
        target,
        message,
        thread: options.thread,
        input_mode: options.input_mode,
        queue_policy: options.queue_policy,
        approval_action: options.approval_action,
        condition: options.condition,
        tail_bytes: options.tail_bytes,
        timeout: options.timeout,
        output_echo_guard: None,
        target_scope: options.target_scope,
        orchestration: options.orchestration,
    })
}

fn send_message_and_watch_condition_with_output_echo_guard(
    request: SendAndWatchRequest<'_>,
) -> io::Result<AskAgentResponse> {
    let mut initial = agent_watch(
        request.target,
        None,
        None,
        vec![
            "status".to_string(),
            "transcript".to_string(),
            "output".to_string(),
            "delivery".to_string(),
        ],
        request.tail_bytes.or(Some(4096)),
        false,
        Duration::from_secs(5),
    )?;
    let mut sent = send_message_with_delivery_and_scope_options(
        request.target,
        request.message,
        SendMessageDeliveryOptions {
            thread: request.thread,
            input_mode: request.input_mode,
            queue_policy: request.queue_policy,
            approval_action: request.approval_action,
            target_scope: request.target_scope,
            timeout: request.timeout,
            orchestration: request.orchestration,
        },
    )?;
    let started_at = Instant::now();
    if request.condition == "status:idle" {
        if let Some(interaction_id) = native_provider_message_id(&sent.delivery).map(str::to_string)
        {
            let completed = wait_for_native_delivery(
                &interaction_id,
                remaining_watch_timeout(
                    request.timeout,
                    started_at,
                    request.target,
                    "native:completed",
                )?,
            )?;
            for detail in &mut sent.delivery {
                if detail.message_id.as_deref() == Some(interaction_id.as_str()) {
                    detail.delivery_state = "provider_applied".to_string();
                    detail.delivery_phase = Some("completed".to_string());
                    detail.observed_state = Some("provider_turn_completed".to_string());
                    detail.reason = Some(format!(
                        "provider-confirmed native completion via {}",
                        completed.record.transport
                    ));
                }
            }
            initial.agent.status = "idle".to_string();
            initial.delivery.delivery = sent.delivery.clone();
            return Ok(AskAgentResponse {
                request_id: None,
                reply: None,
                delivery: sent.delivery,
                watch_error: None,
                watch: initial,
            });
        }
    }
    let condition = effective_send_watch_condition(request.condition, &sent.delivery);
    let delivery_message_ids = live_delivery_message_ids(&sent.delivery);
    let condition_since = if !delivery_message_ids.is_empty()
        && condition_requires_delivery_submission(&condition)
    {
        match wait_for_delivery_submission(
            request.target,
            &initial.cursor,
            &delivery_message_ids,
            request.tail_bytes,
            remaining_watch_timeout(request.timeout, started_at, request.target, &condition)?,
        )? {
            DeliverySubmissionObservation::Submitted { cursor, delivery } => {
                merge_delivery_updates(&mut sent.delivery, delivery);
                cursor
            }
            DeliverySubmissionObservation::Terminal { watch, delivery } => {
                let watch_error = terminal_delivery_watch_error(&delivery);
                merge_delivery_updates(&mut sent.delivery, delivery);
                return Ok(AskAgentResponse {
                    request_id: None,
                    reply: None,
                    delivery: sent.delivery,
                    watch_error: Some(watch_error),
                    watch,
                });
            }
            DeliverySubmissionObservation::TimedOut { watch, delivery } => {
                merge_delivery_updates(&mut sent.delivery, delivery);
                return Ok(AskAgentResponse {
                        request_id: None,
                        reply: None,
                        delivery: sent.delivery,
                        watch_error: Some(WatchEvidenceError {
                            code: "delivery_submission_timeout".to_string(),
                            message: format!(
                                "timed out before delivery {} produced submit-start evidence; provider submission is not confirmed",
                                delivery_message_ids.join(",")
                            ),
                        }),
                        watch,
                    });
            }
        }
    } else {
        initial.cursor.clone()
    };
    let watch_request = AgentWatchRequest {
        target: request.target,
        since: Some(&condition_since),
        until: Some(&condition),
        include: vec![
            "status".to_string(),
            "transcript".to_string(),
            "output".to_string(),
            "delivery".to_string(),
        ],
        tail_bytes: request.tail_bytes,
        follow: false,
        timeout: remaining_watch_timeout(request.timeout, started_at, request.target, &condition)?,
        output_echo_guard: request.output_echo_guard,
    };
    let watch = match agent_watch_with_output_echo_guard(watch_request) {
        Ok(watch) => watch,
        Err(error) if is_watch_timeout(&error) => {
            let watch = agent_watch(
                request.target,
                Some(&condition_since),
                None,
                vec![
                    "status".to_string(),
                    "transcript".to_string(),
                    "output".to_string(),
                    "delivery".to_string(),
                    "events".to_string(),
                ],
                request.tail_bytes,
                false,
                Duration::from_secs(5),
            )?;
            let delivery = matching_delivery_details(&watch, &delivery_message_ids);
            merge_delivery_updates(&mut sent.delivery, delivery);
            return Ok(AskAgentResponse {
                request_id: None,
                reply: None,
                delivery: sent.delivery,
                watch_error: Some(WatchEvidenceError {
                    code: "watch_timeout".to_string(),
                    message: error.to_string(),
                }),
                watch,
            });
        }
        Err(error) => return Err(error),
    };
    Ok(AskAgentResponse {
        request_id: None,
        reply: None,
        delivery: sent.delivery,
        watch_error: None,
        watch,
    })
}

fn is_watch_timeout(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| {
        inner.downcast_ref::<WatchTimeoutError>().is_some()
            || inner
                .downcast_ref::<ControlEndpointError>()
                .is_some_and(|error| error.code() == "watch_timeout")
    })
}

fn native_provider_message_id(delivery: &[DeliveryDetail]) -> Option<&str> {
    delivery.iter().find_map(|detail| {
        (detail.runtime_state == "native_provider_session"
            && detail.delivery_state == "provider_accepted")
            .then_some(detail.message_id.as_deref())
            .flatten()
    })
}

fn wait_for_native_delivery(
    interaction_id: &str,
    timeout: Duration,
) -> io::Result<NativeDeliveryInspectResponse> {
    let started_at = Instant::now();
    loop {
        let delivery = delivery_get(interaction_id, 64)?;
        if delivery.record.phase == NativeDeliveryPhase::Completed {
            return Ok(delivery);
        }
        if delivery.record.phase.is_terminal() {
            return Err(io::Error::other(format!(
                "native delivery {interaction_id} ended as {:?}: {}",
                delivery.record.phase,
                delivery
                    .record
                    .detail
                    .as_deref()
                    .unwrap_or("no provider detail")
            )));
        }
        if started_at.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                WatchTimeoutError::new(interaction_id, "native:completed", "unknown"),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn build_runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| io::Error::other(e.to_string()))
}

fn timeout_block(
    runtime: &tokio::runtime::Runtime,
    operation: ControlOperation,
    fut: impl std::future::Future<Output = io::Result<serde_json::Value>>,
) -> io::Result<serde_json::Value> {
    let timeout = operation_timeout(&operation);
    match runtime.block_on(async { tokio::time::timeout(timeout, fut).await }) {
        Ok(result) => result,
        Err(_) => match operation {
            ControlOperation::AgentWatch { target, until, .. } => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                WatchTimeoutError::new(&target, &until, "unknown"),
            )),
            ControlOperation::Ask { target, .. } => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                WatchTimeoutError::new(&target, "reply", "unknown"),
            )),
            _ => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Wardian control endpoint timed out",
            )),
        },
    }
}

fn operation_timeout(operation: &ControlOperation) -> Duration {
    match operation {
        ControlOperation::AgentList => CONTROL_TIMEOUT,
        ControlOperation::AgentDoctor => CONTROL_DIAGNOSTIC_TIMEOUT,
        ControlOperation::ConversationList
        | ControlOperation::ConversationShow
        | ControlOperation::InboxList
        | ControlOperation::ArtifactShow
        | ControlOperation::ArtifactReviewShow
        | ControlOperation::WatchlistsChanged => CONTROL_TIMEOUT,
        ControlOperation::AgentDelete
        | ControlOperation::AgentRename
        | ControlOperation::AgentRestart
        | ControlOperation::AgentPause
        | ControlOperation::AgentResume
        | ControlOperation::AgentModels
        | ControlOperation::AgentSpawn
        | ControlOperation::AgentUpdate
        | ControlOperation::AgentClone
        | ControlOperation::AgentWorktreeEnable
        | ControlOperation::AgentWorktreeJoin
        | ControlOperation::AgentWorktreeDisable
        | ControlOperation::AutomationRun
        | ControlOperation::ArtifactPresent
        | ControlOperation::SubmitReply
        | ControlOperation::NotifyCreate
        | ControlOperation::TopologyMutate => CONTROL_MUTATION_TIMEOUT,
        ControlOperation::SendMessage { requested } => watch_timeout_for(*requested),
        ControlOperation::AgentWorktreeList => CONTROL_GIT_DISCOVERY_TIMEOUT,
        ControlOperation::Ask { requested, .. } => watch_timeout_for(*requested),
        ControlOperation::AgentWatch { requested, .. } => watch_timeout_for(*requested),
        ControlOperation::NotifyWait { requested } => watch_timeout_for(*requested),
        ControlOperation::Browser { requested } => watch_timeout_for(*requested),
    }
}

fn watch_timeout_for(requested: Duration) -> Duration {
    requested + Duration::from_secs(5)
}

// ---------------------------------------------------------------------------
// wardian browser
// ---------------------------------------------------------------------------

/// Budget for a browser call that does not block on a page condition.
const BROWSER_TIMEOUT: Duration = Duration::from_secs(45);

/// Sends one browser control request and decodes its response.
fn browser_request<T: serde::de::DeserializeOwned>(
    request: ControlRequest,
    requested: Duration,
) -> io::Result<T> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::Browser { requested },
        send_request(request),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))
}

/// The agent this terminal belongs to, used to scope `browser open` by default.
pub fn current_session_id() -> Option<String> {
    std::env::var("WARDIAN_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[allow(clippy::too_many_arguments)]
pub fn browser_open(
    url: Option<String>,
    agent: Option<String>,
    workspace: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    detached: bool,
    blank: bool,
) -> io::Result<BrowserSessionSummary> {
    browser_request(
        ControlRequest::BrowserOpen {
            url,
            agent,
            workspace,
            width,
            height,
            detached,
            blank,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_list() -> io::Result<Vec<BrowserSessionSummary>> {
    browser_request(ControlRequest::BrowserList, BROWSER_TIMEOUT)
}

pub fn browser_close(target: &str) -> io::Result<serde_json::Value> {
    browser_request(
        ControlRequest::BrowserClose {
            target: target.to_string(),
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_navigate(target: &str, action: &str) -> io::Result<BrowserSessionSummary> {
    browser_request(
        ControlRequest::BrowserNavigate {
            target: target.to_string(),
            action: action.to_string(),
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_get(
    target: &str,
    field: &str,
    selector: Option<String>,
) -> io::Result<BrowserGetResult> {
    browser_request(
        ControlRequest::BrowserGet {
            target: target.to_string(),
            field: field.to_string(),
            selector,
        },
        BROWSER_TIMEOUT,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn browser_wait(
    target: &str,
    load_state: Option<String>,
    selector: Option<String>,
    text: Option<String>,
    url_contains: Option<String>,
    function: Option<String>,
    timeout_ms: Option<u64>,
) -> io::Result<BrowserSessionSummary> {
    // The client budget must outlast the server's own wait, or the CLI would
    // report a transport timeout instead of the page condition that failed.
    let requested = Duration::from_millis(timeout_ms.unwrap_or(15_000));
    browser_request(
        ControlRequest::BrowserWait {
            target: target.to_string(),
            load_state,
            selector,
            text,
            url_contains,
            function,
            timeout_ms,
        },
        requested,
    )
}

pub fn browser_snapshot(target: &str, interactive: bool) -> io::Result<PageSnapshot> {
    browser_request(
        ControlRequest::BrowserSnapshot {
            target: target.to_string(),
            interactive,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_act(
    target: &str,
    element_ref: &str,
    action: &str,
    value: Option<String>,
    snapshot_after: bool,
) -> io::Result<BrowserActionResult> {
    browser_request(
        ControlRequest::BrowserAct {
            target: target.to_string(),
            element_ref: element_ref.to_string(),
            action: action.to_string(),
            value,
            snapshot_after,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_screenshot(
    target: &str,
    path: &str,
    full_page: bool,
) -> io::Result<BrowserScreenshotResult> {
    browser_request(
        ControlRequest::BrowserScreenshot {
            target: target.to_string(),
            path: path.to_string(),
            full_page,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_viewport(
    target: &str,
    width: Option<u32>,
    height: Option<u32>,
    reset: bool,
) -> io::Result<BrowserSessionSummary> {
    browser_request(
        ControlRequest::BrowserViewport {
            target: target.to_string(),
            width,
            height,
            reset,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_eval(target: &str, expression: &str) -> io::Result<serde_json::Value> {
    browser_request(
        ControlRequest::BrowserEval {
            target: target.to_string(),
            expression: expression.to_string(),
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_console(
    target: &str,
    level: Option<String>,
    clear: bool,
) -> io::Result<Vec<ConsoleEntry>> {
    browser_request(
        ControlRequest::BrowserConsole {
            target: target.to_string(),
            level,
            clear,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_network(
    target: &str,
    action: wardian_core::browser::NetworkAction,
) -> io::Result<serde_json::Value> {
    browser_request(
        ControlRequest::BrowserNetwork {
            target: target.to_string(),
            action,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_cookies(
    target: &str,
    action: wardian_core::browser::CookieAction,
) -> io::Result<Vec<wardian_core::browser::BrowserCookie>> {
    browser_request(
        ControlRequest::BrowserCookies {
            target: target.to_string(),
            action,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_storage(
    target: &str,
    area: wardian_core::browser::StorageArea,
    action: wardian_core::browser::StorageAction,
) -> io::Result<serde_json::Value> {
    browser_request(
        ControlRequest::BrowserStorage {
            target: target.to_string(),
            area,
            action,
        },
        BROWSER_TIMEOUT,
    )
}

pub fn browser_downloads(
    target: &str,
    clear: bool,
) -> io::Result<Vec<wardian_core::browser::DownloadRecord>> {
    browser_request(
        ControlRequest::BrowserDownloads {
            target: target.to_string(),
            clear,
        },
        BROWSER_TIMEOUT,
    )
}

fn current_message_origin() -> Option<MessageOrigin> {
    std::env::var("WARDIAN_SESSION_ID")
        .ok()
        .map(|session_id| session_id.trim().to_string())
        .filter(|session_id| !session_id.is_empty())
        .map(|session_id| MessageOrigin::WardianAgent { session_id })
}

fn ask_prompt_echo_guard<'a>(condition: &str, message: &'a str) -> Option<&'a str> {
    let token = condition.strip_prefix("output:")?;
    (!token.is_empty() && message.contains(token)).then_some(message)
}

/// Returns the live-surface message IDs whose eventual provider turn belongs
/// to this send. Both immediately submitted and mailbox-queued sends need an
/// exact delivery anchor before a status, output, or turn-completion watch can
/// be trusted.
fn live_delivery_message_ids(delivery: &[DeliveryDetail]) -> Vec<String> {
    delivery
        .iter()
        .filter(|detail| is_live_message_delivery(detail))
        .filter_map(|detail| detail.message_id.clone())
        .collect()
}

fn is_live_message_delivery(detail: &DeliveryDetail) -> bool {
    detail.runtime_state != "headless_process"
        && detail.input_mode == MessageInputMode::Message
        && detail.delivery_state != "failed"
}

fn condition_requires_delivery_submission(condition: &str) -> bool {
    condition.starts_with("output:")
        || condition.starts_with("status:")
        || condition.starts_with("event:")
}

/// A headless delivery is synchronous: by the time `send` returns with
/// `provider_applied`, there is no live session that can transition to Idle.
/// Preserve the familiar `send --wait-until idle` contract by waiting for that
/// specific delivery completion instead of fabricating an Idle status for an
/// offline agent.
fn effective_send_watch_condition(condition: &str, delivery: &[DeliveryDetail]) -> String {
    if condition == "status:idle" && delivery.iter().any(is_completed_headless_delivery) {
        "delivery:provider_applied".to_string()
    } else if condition == "status:idle" && delivery.iter().any(is_live_message_delivery) {
        "event:turn_completed".to_string()
    } else {
        condition.to_string()
    }
}

fn is_completed_headless_delivery(detail: &DeliveryDetail) -> bool {
    detail.runtime_state == "headless_process" && detail.delivery_state == "provider_applied"
}

enum DeliverySubmissionObservation {
    Submitted {
        cursor: String,
        delivery: Vec<DeliveryDetail>,
    },
    Terminal {
        watch: AgentWatchResponse,
        delivery: Vec<DeliveryDetail>,
    },
    TimedOut {
        watch: AgentWatchResponse,
        delivery: Vec<DeliveryDetail>,
    },
}

fn wait_for_delivery_submission(
    target: &str,
    since: &str,
    message_ids: &[String],
    tail_bytes: Option<usize>,
    timeout: Duration,
) -> io::Result<DeliverySubmissionObservation> {
    let started_at = Instant::now();
    let mut since_cursor = since.to_string();
    let mut observed_delivery = Vec::new();

    loop {
        let request = AgentWatchRequest {
            target,
            since: Some(&since_cursor),
            until: Some("event:delivery"),
            include: vec![
                "status".to_string(),
                "transcript".to_string(),
                "output".to_string(),
                "delivery".to_string(),
                "events".to_string(),
            ],
            tail_bytes,
            follow: false,
            timeout: remaining_watch_timeout(
                timeout,
                started_at,
                target,
                "delivery:submit_started",
            )?,
            output_echo_guard: None,
        };
        let (watch, timed_out) = match agent_watch_with_output_echo_guard(request) {
            Ok(watch) => (watch, false),
            Err(error) if is_watch_timeout(&error) => (
                agent_watch(
                    target,
                    Some(&since_cursor),
                    None,
                    vec![
                        "status".to_string(),
                        "transcript".to_string(),
                        "output".to_string(),
                        "delivery".to_string(),
                        "events".to_string(),
                    ],
                    tail_bytes,
                    false,
                    Duration::from_secs(5),
                )?,
                true,
            ),
            Err(error) => return Err(error),
        };
        let updates = matching_delivery_details(&watch, message_ids);
        merge_delivery_updates(&mut observed_delivery, updates);
        if observed_delivery.iter().any(delivery_is_terminal) {
            return Ok(DeliverySubmissionObservation::Terminal {
                watch,
                delivery: observed_delivery,
            });
        }
        if let Some(cursor) = matching_delivery_event_cursor(&watch.events, message_ids) {
            return Ok(DeliverySubmissionObservation::Submitted {
                cursor,
                delivery: observed_delivery,
            });
        }
        if observed_delivery
            .iter()
            .any(|detail| delivery_crossed_submission(detail, message_ids))
        {
            return Ok(DeliverySubmissionObservation::Submitted {
                cursor: watch.cursor,
                delivery: observed_delivery,
            });
        }
        if timed_out || started_at.elapsed() >= timeout {
            return Ok(DeliverySubmissionObservation::TimedOut {
                watch,
                delivery: observed_delivery,
            });
        }
        since_cursor = watch.cursor;
    }
}

fn matching_delivery_details(
    watch: &AgentWatchResponse,
    message_ids: &[String],
) -> Vec<DeliveryDetail> {
    let event_details = watch.events.iter().filter_map(|event| {
        (event.kind == "delivery")
            .then(|| serde_json::from_value::<DeliveryDetail>(event.payload.clone()).ok())
            .flatten()
    });
    event_details
        .chain(watch.delivery.delivery.iter().cloned())
        .filter(|detail| {
            detail
                .message_id
                .as_ref()
                .is_some_and(|id| message_ids.iter().any(|message_id| message_id == id))
        })
        .collect()
}

fn merge_delivery_updates(current: &mut Vec<DeliveryDetail>, updates: Vec<DeliveryDetail>) {
    for update in updates {
        if let Some(existing) = current
            .iter_mut()
            .find(|detail| detail.message_id.is_some() && detail.message_id == update.message_id)
        {
            *existing = update;
        } else {
            current.push(update);
        }
    }
}

fn delivery_is_terminal(detail: &DeliveryDetail) -> bool {
    detail.error.is_some()
        || matches!(
            detail.delivery_state.as_str(),
            "failed" | "cancelled" | "expired" | "withdrawn" | "superseded"
        )
}

fn delivery_crossed_submission(detail: &DeliveryDetail, message_ids: &[String]) -> bool {
    detail
        .message_id
        .as_ref()
        .is_some_and(|id| message_ids.iter().any(|message_id| message_id == id))
        && matches!(
            detail.delivery_state.as_str(),
            "submit_started" | "submit_sent_unconfirmed" | "provider_accepted" | "provider_applied"
        )
}

fn terminal_delivery_watch_error(delivery: &[DeliveryDetail]) -> WatchEvidenceError {
    delivery
        .iter()
        .rev()
        .find_map(|detail| detail.error.as_ref())
        .map_or_else(
            || WatchEvidenceError {
                code: "delivery_terminal".to_string(),
                message: "delivery reached a terminal state before provider submission".to_string(),
            },
            |error| WatchEvidenceError {
                code: error.code.clone(),
                message: error.message.clone(),
            },
        )
}

fn matching_delivery_event_cursor(events: &[WatchEvent], message_ids: &[String]) -> Option<String> {
    events.iter().find_map(|event| {
        if event.kind != "delivery" {
            return None;
        }
        let detail = serde_json::from_value::<DeliveryDetail>(event.payload.clone()).ok()?;
        delivery_matches_submit(&detail, message_ids).then(|| event.cursor.clone())
    })
}

fn delivery_matches_submit(detail: &DeliveryDetail, message_ids: &[String]) -> bool {
    detail.delivery_state == "submit_started"
        && detail
            .message_id
            .as_ref()
            .is_some_and(|id| message_ids.iter().any(|queued_id| queued_id == id))
}

fn remaining_watch_timeout(
    timeout: Duration,
    started_at: Instant,
    target: &str,
    condition: &str,
) -> io::Result<Duration> {
    let elapsed = started_at.elapsed();
    if elapsed >= timeout {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            WatchTimeoutError::new(target, condition, "unknown"),
        ));
    }
    Ok(timeout - elapsed)
}

/// One elapsed-time budget for an explicit agent wait, including transport and
/// the initial cursor read for `--next`. Delivery observation has separate rules.
struct WaitBudget {
    started_at: Instant,
    timeout: Duration,
}

impl WaitBudget {
    fn new(timeout: Duration) -> Self {
        Self {
            started_at: Instant::now(),
            timeout,
        }
    }

    fn remaining(&self) -> Option<Duration> {
        self.timeout
            .checked_sub(self.started_at.elapsed())
            .filter(|remaining| !remaining.is_zero())
    }
}

fn wait_transport_timeout() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "Wardian control endpoint timed out",
    )
}

/// Limit the actual IPC future, not just the interval between requests. Runtime
/// construction and previous reads have already consumed part of the budget.
fn wait_transport<T>(
    runtime: &tokio::runtime::Runtime,
    budget: &WaitBudget,
    operation_limit: Duration,
    future: impl std::future::Future<Output = io::Result<T>>,
) -> io::Result<T> {
    let remaining = budget.remaining().ok_or_else(wait_transport_timeout)?;
    let result = runtime
        .block_on(async { tokio::time::timeout(remaining.min(operation_limit), future).await })
        .map_err(|_| wait_transport_timeout())?;
    if result.is_ok() && budget.remaining().is_none() {
        return Err(wait_transport_timeout());
    }
    result
}

fn wait_agent_until_next_with<F>(
    target: &str,
    until: &str,
    timeout: Duration,
    mut watch: F,
) -> io::Result<AgentWatchResponse>
where
    F: FnMut(Option<&str>, Option<&str>, &WaitBudget) -> io::Result<AgentWatchResponse>,
{
    let budget = WaitBudget::new(timeout);
    let timeout_error = |status: &str| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            WatchTimeoutError::new(target, &format!("status:{until}"), status),
        )
    };
    budget.remaining().ok_or_else(|| timeout_error("unknown"))?;
    let initial = watch(None, None, &budget).map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            timeout_error("unknown")
        } else {
            error
        }
    })?;
    budget
        .remaining()
        .ok_or_else(|| timeout_error(&initial.agent.status))?;
    let result = watch(
        Some(&initial.cursor),
        Some(&format!("status:{until}")),
        &budget,
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            timeout_error(&initial.agent.status)
        } else {
            error
        }
    })?;
    budget
        .remaining()
        .ok_or_else(|| timeout_error(&result.agent.status))?;
    Ok(result)
}

fn wait_agent_until_after_snapshot(
    target: &str,
    until: &str,
    timeout: Duration,
    initial_snapshot: Option<AgentIdentity>,
) -> io::Result<AgentIdentity> {
    wait_agent_until_after_snapshot_with(
        target,
        until,
        timeout,
        initial_snapshot,
        wait_target_snapshot,
        std::thread::sleep,
    )
}

fn wait_agent_until_after_snapshot_with<F, S>(
    target: &str,
    until: &str,
    timeout: Duration,
    initial_snapshot: Option<AgentIdentity>,
    mut snapshot: F,
    mut sleep: S,
) -> io::Result<AgentIdentity>
where
    F: FnMut(&str, &WaitBudget) -> io::Result<AgentIdentity>,
    S: FnMut(Duration),
{
    let budget = WaitBudget::new(timeout);
    let initial_status = initial_snapshot.as_ref().map(|agent| agent.status.as_str());
    let mut observed_away_from_initial = initial_status.is_none_or(|status| status != until);
    let mut last_status = initial_status.unwrap_or("unknown").to_string();
    let timeout_error = |status: &str| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            WaitTimeoutError::new(target, until, status),
        )
    };

    loop {
        budget
            .remaining()
            .ok_or_else(|| timeout_error(&last_status))?;
        let agent = snapshot(target, &budget).map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut && budget.remaining().is_none() {
                timeout_error(&last_status)
            } else {
                error
            }
        })?;
        let status = agent.status.as_str();
        last_status = status.to_string();
        // A slow snapshot is not a timely observation, even when it matches.
        budget.remaining().ok_or_else(|| timeout_error(status))?;

        if initial_status == Some(until) && status != until {
            observed_away_from_initial = true;
        }

        if status == until
            && (observed_away_from_initial
                || initial_snapshot
                    .as_ref()
                    .is_some_and(|initial| status_marker_changed(initial, &agent)))
        {
            return Ok(agent);
        }

        if matches!(status, "error" | "off") && status != until {
            return Err(io::Error::other(format!(
                "agent {target} reached terminal status {status} before {until}"
            )));
        }

        let remaining = budget.remaining().ok_or_else(|| timeout_error(status))?;
        sleep(Duration::from_millis(250).min(remaining));
    }
}

fn status_marker_changed(initial: &AgentIdentity, current: &AgentIdentity) -> bool {
    initial.uuid == current.uuid
        && initial.status == current.status
        && initial.last_status_at != current.last_status_at
}

fn wait_target_snapshot(target: &str, budget: &WaitBudget) -> io::Result<AgentIdentity> {
    if target == "all" || target.starts_with("class:") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "wait requires a single agent name or uuid",
        ));
    }

    let runtime = build_runtime()?;
    let value = wait_transport(
        &runtime,
        budget,
        CONTROL_TIMEOUT,
        send_request(ControlRequest::AgentList),
    )?;
    let response: AgentListResponse =
        serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))?;
    response
        .agents
        .into_iter()
        .find(|agent| agent.uuid == target || agent.name == target)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                WaitTargetNotFoundError::new(target),
            )
        })
}

async fn send_request(req: ControlRequest) -> io::Result<serde_json::Value> {
    #[cfg(windows)]
    {
        send_request_windows(req).await
    }
    #[cfg(unix)]
    {
        send_request_unix(req).await
    }
}

#[cfg(windows)]
async fn send_request_windows(req: ControlRequest) -> io::Result<serde_json::Value> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let pipe_name = wardian_core::control::pipe_name()
        .ok_or_else(|| io::Error::other("could not resolve Wardian control pipe"))?;
    let mut stream = ClientOptions::new().open(pipe_name)?;
    exchange_json(&mut stream, req).await
}

#[cfg(unix)]
async fn send_request_unix(req: ControlRequest) -> io::Result<serde_json::Value> {
    use tokio::net::UnixStream;

    let socket_path = wardian_core::control::socket_path()
        .ok_or_else(|| io::Error::other("could not resolve Wardian control socket"))?;
    let mut stream = UnixStream::connect(socket_path).await?;
    exchange_json(&mut stream, req).await
}

async fn exchange_json<T>(stream: &mut T, req: ControlRequest) -> io::Result<serde_json::Value>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let request = serde_json::to_string(&req).map_err(|e| io::Error::other(e.to_string()))?;
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut line).await?;

    // Detect backend error envelope {"error": {...}}
    let value: serde_json::Value =
        serde_json::from_str(&line).map_err(|e| io::Error::other(e.to_string()))?;
    if let Some(err) = value.get("error") {
        let code = err
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap_or("request_failed");
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        let endpoint_error = err.get("details").cloned().map_or_else(
            || ControlEndpointError::new(code, msg),
            |details| ControlEndpointError::with_details(code, msg, details),
        );
        return Err(io::Error::other(endpoint_error));
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::control::MessageOrigin;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    #[test]
    fn agent_mutations_use_longer_control_timeout() {
        assert!(operation_timeout(&ControlOperation::AgentSpawn) > CONTROL_TIMEOUT);
        assert_eq!(
            operation_timeout(&ControlOperation::AgentUpdate),
            CONTROL_MUTATION_TIMEOUT
        );
        assert_eq!(
            operation_timeout(&ControlOperation::AgentModels),
            CONTROL_MUTATION_TIMEOUT
        );
        assert!(operation_timeout(&ControlOperation::AgentClone) > CONTROL_TIMEOUT);
    }

    #[test]
    fn worktree_mutations_use_longer_control_timeout() {
        assert_eq!(
            operation_timeout(&ControlOperation::AgentWorktreeEnable),
            CONTROL_MUTATION_TIMEOUT
        );
        assert_eq!(
            operation_timeout(&ControlOperation::AgentWorktreeJoin),
            CONTROL_MUTATION_TIMEOUT
        );
        assert_eq!(
            operation_timeout(&ControlOperation::AgentWorktreeDisable),
            CONTROL_MUTATION_TIMEOUT
        );
    }

    #[test]
    fn worktree_list_uses_git_discovery_timeout() {
        let timeout = operation_timeout(&ControlOperation::AgentWorktreeList);

        assert!(timeout > CONTROL_TIMEOUT);
        assert!(timeout < CONTROL_MUTATION_TIMEOUT);
    }

    #[test]
    fn agent_doctor_outlives_fast_read_timeout() {
        let timeout = operation_timeout(&ControlOperation::AgentDoctor);

        assert_eq!(timeout, CONTROL_DIAGNOSTIC_TIMEOUT);
        assert!(timeout > CONTROL_TIMEOUT);
    }

    #[test]
    fn send_message_uses_requested_timeout_plus_slack() {
        let requested = Duration::from_secs(30);
        assert_eq!(
            operation_timeout(&ControlOperation::SendMessage { requested }),
            watch_timeout_for(requested)
        );
    }

    #[test]
    fn current_message_origin_uses_wardian_session_id() {
        let _guard = env_lock();
        std::env::set_var("WARDIAN_SESSION_ID", "source-1");

        assert_eq!(
            current_message_origin(),
            Some(MessageOrigin::WardianAgent {
                session_id: "source-1".to_string()
            })
        );

        std::env::remove_var("WARDIAN_SESSION_ID");
    }

    #[test]
    fn current_message_origin_ignores_blank_session_id() {
        let _guard = env_lock();
        std::env::set_var("WARDIAN_SESSION_ID", "   ");

        assert_eq!(current_message_origin(), None);

        std::env::remove_var("WARDIAN_SESSION_ID");
    }

    #[test]
    fn agent_list_keeps_short_control_timeout() {
        assert_eq!(
            operation_timeout(&ControlOperation::AgentList),
            CONTROL_TIMEOUT
        );
    }

    #[test]
    fn conversation_control_operations_keep_short_timeout() {
        assert_eq!(
            operation_timeout(&ControlOperation::ConversationList),
            CONTROL_TIMEOUT
        );
        assert_eq!(
            operation_timeout(&ControlOperation::ConversationShow),
            CONTROL_TIMEOUT
        );
    }

    #[test]
    fn agent_watch_operation_timeout_includes_requested_timeout_plus_slack() {
        let requested = Duration::from_secs(30);
        let actual = operation_timeout(&ControlOperation::AgentWatch {
            requested,
            target: "Wardian-Codex".to_string(),
            until: "output:OK".to_string(),
        });

        assert!(actual > requested);
        assert!(actual < requested + Duration::from_secs(10));
    }

    #[test]
    fn ask_output_condition_sets_prompt_echo_guard() {
        assert_eq!(
            ask_prompt_echo_guard("output:AUTO_TEST_2_DONE", "Say AUTO_TEST_2_DONE"),
            Some("Say AUTO_TEST_2_DONE")
        );
        assert_eq!(ask_prompt_echo_guard("status:idle", "Say DONE"), None);
    }

    fn delivery_detail(state: &str, message_id: Option<&str>) -> DeliveryDetail {
        DeliveryDetail {
            uuid: "agent-1".to_string(),
            name: "reviewer-a1".to_string(),
            provider: "mock".to_string(),
            runtime_state: "target_action_required".to_string(),
            delivery_state: state.to_string(),
            input_mode: MessageInputMode::Message,
            queue_policy: QueuePolicy::QueueIfBusy,
            message_id: message_id.map(str::to_string),
            delivery_phase: None,
            observed_state: None,
            reason: None,
            profile: None,
            error: None,
        }
    }

    #[test]
    fn live_delivery_message_ids_returns_direct_and_queued_message_ids() {
        let delivery = vec![
            delivery_detail("queued", Some("msg_1")),
            delivery_detail("submit_sent_unconfirmed", Some("int_2")),
            delivery_detail("queued", None),
            DeliveryDetail {
                runtime_state: "headless_process".to_string(),
                ..delivery_detail("provider_applied", Some("int_headless"))
            },
        ];

        assert_eq!(live_delivery_message_ids(&delivery), vec!["msg_1", "int_2"]);
    }

    #[test]
    fn native_provider_wait_anchors_to_the_exact_interaction() {
        let delivery = vec![DeliveryDetail {
            runtime_state: "native_provider_session".to_string(),
            delivery_state: "provider_accepted".to_string(),
            ..delivery_detail("ignored", Some("int_native"))
        }];

        assert_eq!(native_provider_message_id(&delivery), Some("int_native"));
        assert_eq!(
            native_provider_message_id(&[DeliveryDetail {
                runtime_state: "headless_process".to_string(),
                delivery_state: "provider_applied".to_string(),
                ..delivery_detail("ignored", Some("int_headless"))
            }]),
            None
        );
    }

    #[test]
    fn matching_delivery_event_cursor_uses_same_message_id_and_state() {
        let events = vec![
            wardian_core::control::WatchEvent {
                cursor: "agent-1:1".to_string(),
                kind: "delivery".to_string(),
                payload: serde_json::json!(delivery_detail("submit_started", Some("msg_other"))),
            },
            wardian_core::control::WatchEvent {
                cursor: "agent-1:2".to_string(),
                kind: "delivery".to_string(),
                payload: serde_json::json!(delivery_detail("submit_started", Some("msg_1"))),
            },
        ];

        assert_eq!(
            matching_delivery_event_cursor(&events, &["msg_1".to_string()]).as_deref(),
            Some("agent-1:2")
        );
    }

    #[test]
    fn matching_delivery_details_keeps_exact_failed_delivery_classification() {
        let failed = DeliveryDetail {
            delivery_state: "failed".to_string(),
            delivery_phase: Some("payload_apply_unconfirmed".to_string()),
            error: Some(wardian_core::control::DeliveryErrorDetail {
                code: "payload_apply_unconfirmed".to_string(),
                message: "Return was not sent".to_string(),
            }),
            ..delivery_detail("queued", Some("msg_1"))
        };
        let watch = AgentWatchResponse {
            schema: 1,
            agent: wardian_core::control::WatchAgentSnapshot {
                uuid: "agent-1".to_string(),
                name: "agent-1".to_string(),
                provider: "codex".to_string(),
                status: "idle".to_string(),
                last_status_at: None,
            },
            cursor: "agent-1:2".to_string(),
            events: vec![WatchEvent {
                cursor: "agent-1:2".to_string(),
                kind: "delivery".to_string(),
                payload: serde_json::to_value(&failed).unwrap(),
            }],
            output: wardian_core::control::WatchOutput {
                cursor: "agent-1:2".to_string(),
                text: String::new(),
                truncated: false,
                omitted_bytes: 0,
            },
            transcript: None,
            raw_output: None,
            delivery: wardian_core::control::WatchDeliverySnapshot {
                delivery: Vec::new(),
            },
        };

        let details = matching_delivery_details(&watch, &["msg_1".to_string()]);
        assert_eq!(details, vec![failed]);
        assert!(details.iter().any(delivery_is_terminal));
        assert_eq!(
            terminal_delivery_watch_error(&details).code,
            "payload_apply_unconfirmed"
        );
    }

    #[test]
    fn delivery_update_replaces_queued_state_with_terminal_evidence() {
        let mut current = vec![delivery_detail("queued", Some("msg_1"))];
        let failed = DeliveryDetail {
            delivery_state: "failed".to_string(),
            error: Some(wardian_core::control::DeliveryErrorDetail {
                code: "payload_apply_unconfirmed".to_string(),
                message: "Return was not sent".to_string(),
            }),
            ..delivery_detail("queued", Some("msg_1"))
        };

        merge_delivery_updates(&mut current, vec![failed.clone()]);

        assert_eq!(current, vec![failed]);
    }

    #[test]
    fn backend_watch_timeout_is_recognized_for_structured_fallback() {
        let error = io::Error::other(ControlEndpointError::new(
            "watch_timeout",
            "watch condition timed out",
        ));

        assert!(is_watch_timeout(&error));
    }

    #[test]
    fn delivery_submission_prewait_applies_to_target_behavior_conditions() {
        assert!(condition_requires_delivery_submission("output:DONE"));
        assert!(condition_requires_delivery_submission("status:idle"));
        assert!(condition_requires_delivery_submission(
            "event:turn_completed"
        ));
        assert!(!condition_requires_delivery_submission(
            "delivery:submit_sent_unconfirmed"
        ));
        assert!(!condition_requires_delivery_submission("delivery:queued"));
    }

    #[test]
    fn headless_idle_wait_uses_the_delivery_completion_event() {
        let delivery = vec![DeliveryDetail {
            runtime_state: "headless_process".to_string(),
            ..delivery_detail("provider_applied", Some("int_1"))
        }];

        assert_eq!(
            effective_send_watch_condition("status:idle", &delivery),
            "delivery:provider_applied"
        );
        assert_eq!(
            effective_send_watch_condition("status:headless", &delivery),
            "status:headless"
        );
        assert_eq!(
            effective_send_watch_condition(
                "status:idle",
                &[delivery_detail("submit_sent_unconfirmed", Some("int_2"))]
            ),
            "event:turn_completed"
        );
    }

    #[test]
    fn wait_target_rejects_multi_target_selectors() {
        let error =
            wait_target_snapshot("all", &WaitBudget::new(Duration::from_secs(1))).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    fn agent(status: &str, last_status_at: Option<&str>) -> AgentIdentity {
        AgentIdentity {
            name: "reviewer-a1".to_string(),
            uuid: "uuid-1".to_string(),
            description: String::new(),
            class: "Reviewer".to_string(),
            provider: "codex".to_string(),
            status: status.to_string(),
            pid: Some(42),
            started_at: Some("2026-05-07T12:00:00.000Z".to_string()),
            workspace: Some("D:/Development/Wardian".to_string()),
            last_status_at: last_status_at.map(str::to_string),
            status_source: wardian_core::identity::StatusSource::Live,
            visibility: None,
        }
    }

    #[test]
    fn wait_after_send_accepts_fast_return_to_initial_status_when_timestamp_changes() {
        let initial = agent("idle", Some("2026-05-07T12:00:00.000Z"));
        let completed = agent("idle", Some("2026-05-07T12:00:01.000Z"));

        let result = wait_agent_until_after_snapshot_with(
            "reviewer-a1",
            "idle",
            Duration::from_secs(1),
            Some(initial),
            |_, _| Ok(completed.clone()),
            |_| {},
        )
        .unwrap();

        assert_eq!(
            result.last_status_at.as_deref(),
            Some("2026-05-07T12:00:01.000Z")
        );
    }

    #[test]
    fn wait_target_not_found_uses_typed_error() {
        let error = wait_agent_until_after_snapshot_with(
            "ghost",
            "idle",
            Duration::from_secs(1),
            None,
            |_, _| {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    WaitTargetNotFoundError::new("ghost"),
                ))
            },
            |_| {},
        )
        .unwrap_err();

        assert!(error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<WaitTargetNotFoundError>())
            .is_some());
    }

    #[test]
    fn wait_rejects_a_matching_snapshot_received_after_the_budget() {
        let error = wait_agent_until_after_snapshot_with(
            "reviewer-a1",
            "idle",
            Duration::from_millis(30),
            None,
            |_, _| {
                std::thread::sleep(Duration::from_millis(60));
                Ok(agent("idle", None))
            },
            |_| panic!("a late matching snapshot must time out without polling"),
        )
        .unwrap_err();
        let timeout = error
            .get_ref()
            .unwrap()
            .downcast_ref::<WaitTimeoutError>()
            .unwrap();
        assert_eq!(timeout.last_status, "idle");
    }

    #[test]
    fn wait_caps_poll_sleep_and_does_not_start_another_expired_read() {
        let mut reads = 0;
        let mut sleeps = 0;
        let error = wait_agent_until_after_snapshot_with(
            "reviewer-a1",
            "idle",
            Duration::from_millis(100),
            None,
            |_, _| {
                reads += 1;
                Ok(agent("processing", None))
            },
            |duration| {
                sleeps += 1;
                assert!(duration <= Duration::from_millis(100));
                std::thread::sleep(duration);
            },
        )
        .unwrap_err();
        assert_eq!(reads, 1);
        assert_eq!(sleeps, 1);
        assert!(error.get_ref().unwrap().is::<WaitTimeoutError>());
    }

    #[test]
    fn wait_preserves_immediate_matches_and_terminal_status_failures() {
        for status in ["idle", "error", "off"] {
            let result = wait_agent_until_after_snapshot_with(
                "reviewer-a1",
                "idle",
                Duration::from_secs(30),
                None,
                |_, _| Ok(agent(status, None)),
                |_| panic!("matching or terminal snapshots must not poll"),
            );
            if status == "idle" {
                assert_eq!(result.unwrap().status, "idle");
            } else {
                assert!(result.unwrap_err().to_string().contains("terminal status"));
            }
        }
    }

    #[test]
    fn wait_zero_budget_does_not_contact_the_endpoint() {
        let error = wait_agent_until_after_snapshot_with(
            "reviewer-a1",
            "idle",
            Duration::ZERO,
            None,
            |_, _| panic!("zero budget must not start a read"),
            |_| panic!("zero budget must not sleep"),
        )
        .unwrap_err();
        assert!(error.get_ref().unwrap().is::<WaitTimeoutError>());
        let error = wait_agent_until_next_with("reviewer-a1", "idle", Duration::ZERO, |_, _, _| {
            panic!("zero budget must not obtain an initial cursor")
        })
        .unwrap_err();
        assert!(error.get_ref().unwrap().is::<WatchTimeoutError>());
    }

    #[test]
    fn wait_transport_uses_remaining_budget_instead_of_operation_timeout() {
        let runtime = build_runtime().unwrap();
        let budget = WaitBudget::new(Duration::from_millis(10));
        let error = wait_transport(&runtime, &budget, CONTROL_TIMEOUT, async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn wait_preserves_early_endpoint_timeout_but_types_budget_expiration() {
        let runtime = build_runtime().unwrap();
        let early = wait_agent_until_after_snapshot_with(
            "reviewer-a1",
            "idle",
            Duration::from_secs(30),
            None,
            |_, _| Err(wait_transport_timeout()),
            |_| panic!("failed reads must not poll"),
        )
        .unwrap_err();
        assert_eq!(early.kind(), io::ErrorKind::TimedOut);
        assert!(!early.get_ref().unwrap().is::<WaitTimeoutError>());

        let expired = wait_agent_until_after_snapshot_with(
            "reviewer-a1",
            "idle",
            Duration::from_millis(10),
            None,
            |_, budget| {
                wait_transport(
                    &runtime,
                    budget,
                    CONTROL_TIMEOUT,
                    std::future::pending::<io::Result<AgentIdentity>>(),
                )
            },
            |_| panic!("exhausted transport budget must not poll"),
        )
        .unwrap_err();
        assert!(expired.get_ref().unwrap().is::<WaitTimeoutError>());
    }

    fn wait_snapshot(cursor: &str) -> AgentWatchResponse {
        AgentWatchResponse {
            schema: 1,
            agent: wardian_core::control::WatchAgentSnapshot {
                uuid: "uuid-1".into(),
                name: "reviewer-a1".into(),
                provider: "codex".into(),
                status: "idle".into(),
                last_status_at: None,
            },
            cursor: cursor.into(),
            events: Vec::new(),
            output: wardian_core::control::WatchOutput {
                cursor: cursor.into(),
                text: String::new(),
                truncated: false,
                omitted_bytes: 0,
            },
            transcript: None,
            raw_output: None,
            delivery: wardian_core::control::WatchDeliverySnapshot {
                delivery: Vec::new(),
            },
        }
    }

    #[test]
    fn wait_next_initial_snapshot_consumes_the_same_budget() {
        let mut calls = 0;
        let error = wait_agent_until_next_with(
            "reviewer-a1",
            "idle",
            Duration::from_millis(30),
            |since, condition, _| {
                calls += 1;
                assert!(since.is_none());
                assert!(condition.is_none());
                std::thread::sleep(Duration::from_millis(60));
                Ok(wait_snapshot("initial"))
            },
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(error.get_ref().unwrap().is::<WatchTimeoutError>());
    }

    #[test]
    fn wait_next_preserves_cursor_and_reduces_followup_budget() {
        let timeout = Duration::from_secs(30);
        let mut calls = 0;
        let result = wait_agent_until_next_with(
            "reviewer-a1",
            "idle",
            timeout,
            |since, condition, budget| {
                calls += 1;
                if calls == 1 {
                    assert!(since.is_none());
                    assert!(condition.is_none());
                    std::thread::sleep(Duration::from_millis(20));
                    Ok(wait_snapshot("initial"))
                } else {
                    assert_eq!(since, Some("initial"));
                    assert_eq!(condition, Some("status:idle"));
                    assert!(budget.remaining().unwrap() <= timeout - Duration::from_millis(20));
                    Ok(wait_snapshot("next"))
                }
            },
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result.cursor, "next");
    }

    #[test]
    fn wait_next_rejects_a_late_matching_followup() {
        let mut calls = 0;
        let error = wait_agent_until_next_with(
            "reviewer-a1",
            "idle",
            Duration::from_millis(100),
            |since, _, _| {
                calls += 1;
                if since.is_some() {
                    std::thread::sleep(Duration::from_millis(150));
                }
                Ok(wait_snapshot("cursor"))
            },
        )
        .unwrap_err();
        assert_eq!(calls, 2);
        assert!(error.get_ref().unwrap().is::<WatchTimeoutError>());
    }

    #[test]
    fn wait_next_initial_transport_is_cancelled_at_the_shared_deadline() {
        let runtime = build_runtime().unwrap();
        let mut calls = 0;
        let error = wait_agent_until_next_with(
            "reviewer-a1",
            "idle",
            Duration::from_millis(10),
            |_, _, budget| {
                calls += 1;
                wait_transport(
                    &runtime,
                    budget,
                    watch_timeout_for(Duration::from_secs(5)),
                    std::future::pending::<io::Result<AgentWatchResponse>>(),
                )
            },
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(error.get_ref().unwrap().is::<WatchTimeoutError>());
    }

    #[test]
    fn exchange_json_preserves_backend_error_details() {
        let runtime = build_runtime().unwrap();
        runtime.block_on(async {
            let (mut client, mut server) = tokio::io::duplex(4096);
            tokio::spawn(async move {
            let mut line = String::new();
            let mut reader = BufReader::new(&mut server);
            reader.read_line(&mut line).await.unwrap();
            let stream = reader.get_mut();
            stream
                .write_all(
                    br#"{"schema":1,"error":{"code":"request_failed","message":"message delivery failed","details":{"delivery":[{"uuid":"agent-2","name":"CoderTwo","provider":"claude","runtime_state":"restored_without_sender","delivery_state":"failed","error":{"code":"no_input_channel","message":"missing sender"}}]}}}"#,
                )
                .await
                .unwrap();
            stream.write_all(b"\n").await.unwrap();
            });

            let error = exchange_json(&mut client, ControlRequest::AgentList)
                .await
                .unwrap_err();
            let endpoint_error = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<ControlEndpointError>())
                .unwrap();

            assert_eq!(endpoint_error.code(), "request_failed");
            assert_eq!(
                endpoint_error.details().unwrap()["delivery"][0]["runtime_state"],
                "restored_without_sender"
            );
        });
    }
}
