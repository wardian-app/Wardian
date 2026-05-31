use std::{
    fmt, io,
    time::{Duration, Instant},
};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wardian_core::control::{
    AgentListResponse, AgentResponse, AgentWatchResponse, AgentWorktreeListResponse,
    AgentWorktreeMutationResponse, AgentWorktreeSummary, ApprovalAction, AskResponse,
    ControlRequest, DeliveryDetail, MessageInputMode, MessageOrigin, QueuePolicy, ReplyResponse,
    ReplyStatus, SendMessageResponse, StructuredReply, WatchEvent, WatchEvidenceError,
};
use wardian_core::identity::AgentIdentity;

const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);
const CONTROL_GIT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
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
}

pub struct SendMessageAndWatchOptions<'a> {
    pub thread: Option<&'a str>,
    pub input_mode: MessageInputMode,
    pub queue_policy: QueuePolicy,
    pub approval_action: Option<ApprovalAction>,
    pub until: &'a str,
    pub timeout: Duration,
}

pub struct SendMessageAndWatchConditionOptions<'a> {
    pub thread: Option<&'a str>,
    pub input_mode: MessageInputMode,
    pub queue_policy: QueuePolicy,
    pub approval_action: Option<ApprovalAction>,
    pub condition: &'a str,
    pub tail_bytes: Option<usize>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlOperation {
    AgentList,
    AgentKill,
    AgentPause,
    AgentResume,
    AgentSpawn,
    AgentClone,
    AgentWorktreeList,
    AgentWorktreeEnable,
    AgentWorktreeJoin,
    AgentWorktreeDisable,
    SendMessage,
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

pub fn agent_kill(target: &str) -> io::Result<()> {
    let runtime = build_runtime()?;
    timeout_block(
        &runtime,
        ControlOperation::AgentKill,
        send_request(ControlRequest::AgentKill {
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
        }),
    )?;
    let resp: AgentResponse =
        serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(resp.agent)
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

pub fn send_message(
    target: &str,
    message: &str,
    thread: Option<&str>,
) -> io::Result<SendMessageResponse> {
    send_message_with_input_mode(target, message, thread, MessageInputMode::Message)
}

pub fn send_message_with_input_mode(
    target: &str,
    message: &str,
    thread: Option<&str>,
    input_mode: MessageInputMode,
) -> io::Result<SendMessageResponse> {
    send_message_with_delivery_options(
        target,
        message,
        thread,
        input_mode,
        QueuePolicy::QueueIfBusy,
        None,
    )
}

pub fn send_message_with_delivery_options(
    target: &str,
    message: &str,
    thread: Option<&str>,
    input_mode: MessageInputMode,
    queue_policy: QueuePolicy,
    approval_action: Option<ApprovalAction>,
) -> io::Result<SendMessageResponse> {
    let runtime = build_runtime()?;
    let value = timeout_block(
        &runtime,
        ControlOperation::SendMessage,
        send_request(ControlRequest::SendMessage {
            target: target.to_string(),
            message: message.to_string(),
            thread: thread.map(str::to_string),
            input_mode,
            queue_policy,
            approval_action,
            origin: current_message_origin(),
        }),
    )?;
    serde_json::from_value(value).map_err(|e| io::Error::other(e.to_string()))
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

pub fn wait_agent_until_next(
    target: &str,
    until: &str,
    timeout: Duration,
) -> io::Result<AgentWatchResponse> {
    let initial = agent_watch(
        target,
        None,
        None,
        vec!["status".to_string()],
        Some(4096),
        false,
        Duration::from_secs(5),
    )?;
    agent_watch(
        target,
        Some(&initial.cursor),
        Some(&format!("status:{until}")),
        vec!["status".to_string()],
        Some(4096),
        false,
        timeout,
    )
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
) -> io::Result<AskAgentResponse> {
    if condition == "reply" {
        return ask_agent_structured(target, message, thread, tail_bytes, timeout);
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
    })
}

fn ask_agent_structured(
    target: &str,
    message: &str,
    thread: Option<&str>,
    tail_bytes: Option<usize>,
    timeout: Duration,
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
    })
}

fn send_message_and_watch_condition_with_output_echo_guard(
    request: SendAndWatchRequest<'_>,
) -> io::Result<AskAgentResponse> {
    let initial = agent_watch(
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
    let sent = send_message_with_delivery_options(
        request.target,
        request.message,
        request.thread,
        request.input_mode,
        request.queue_policy,
        request.approval_action,
    )?;
    let started_at = Instant::now();
    let queued_message_ids = queued_delivery_message_ids(&sent.delivery);
    let condition_since = if !queued_message_ids.is_empty()
        && condition_requires_queued_delivery_submission(request.condition)
    {
        wait_for_queued_delivery_submission(
            request.target,
            &initial.cursor,
            &queued_message_ids,
            request.tail_bytes,
            remaining_watch_timeout(
                request.timeout,
                started_at,
                request.target,
                request.condition,
            )?,
        )?
    } else {
        initial.cursor.clone()
    };
    let watch = agent_watch_with_output_echo_guard(AgentWatchRequest {
        target: request.target,
        since: Some(&condition_since),
        until: Some(request.condition),
        include: vec![
            "status".to_string(),
            "transcript".to_string(),
            "output".to_string(),
            "delivery".to_string(),
        ],
        tail_bytes: request.tail_bytes,
        follow: false,
        timeout: remaining_watch_timeout(
            request.timeout,
            started_at,
            request.target,
            request.condition,
        )?,
        output_echo_guard: request.output_echo_guard,
    })?;
    Ok(AskAgentResponse {
        request_id: None,
        reply: None,
        delivery: sent.delivery,
        watch_error: None,
        watch,
    })
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
        ControlOperation::AgentKill
        | ControlOperation::AgentPause
        | ControlOperation::AgentResume
        | ControlOperation::AgentSpawn
        | ControlOperation::AgentClone
        | ControlOperation::AgentWorktreeEnable
        | ControlOperation::AgentWorktreeJoin
        | ControlOperation::AgentWorktreeDisable
        | ControlOperation::SendMessage
        | ControlOperation::SubmitReply => CONTROL_MUTATION_TIMEOUT,
        ControlOperation::AgentWorktreeList => CONTROL_GIT_DISCOVERY_TIMEOUT,
        ControlOperation::Ask { requested, .. } => watch_timeout_for(*requested),
        ControlOperation::AgentWatch { requested, .. } => watch_timeout_for(*requested),
    }
}

fn watch_timeout_for(requested: Duration) -> Duration {
    requested + Duration::from_secs(5)
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

fn queued_delivery_message_ids(delivery: &[DeliveryDetail]) -> Vec<String> {
    delivery
        .iter()
        .filter(|detail| detail.delivery_state == "queued")
        .filter_map(|detail| detail.message_id.clone())
        .collect()
}

fn condition_requires_queued_delivery_submission(condition: &str) -> bool {
    condition.starts_with("output:") || condition.starts_with("status:")
}

fn wait_for_queued_delivery_submission(
    target: &str,
    since: &str,
    message_ids: &[String],
    tail_bytes: Option<usize>,
    timeout: Duration,
) -> io::Result<String> {
    let started_at = Instant::now();
    let mut since_cursor = since.to_string();

    loop {
        let watch = agent_watch_with_output_echo_guard(AgentWatchRequest {
            target,
            since: Some(&since_cursor),
            until: Some("delivery:submit_started"),
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
        })?;

        if let Some(cursor) = matching_delivery_event_cursor(&watch.events, message_ids) {
            return Ok(cursor);
        }
        if watch
            .delivery
            .delivery
            .iter()
            .any(|detail| delivery_matches_submit(detail, message_ids))
        {
            return Ok(watch.cursor);
        }
        since_cursor = watch.cursor;
    }
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
    F: FnMut(&str) -> io::Result<AgentIdentity>,
    S: FnMut(Duration),
{
    let started_at = Instant::now();
    let initial_status = initial_snapshot.as_ref().map(|agent| agent.status.as_str());
    let mut observed_away_from_initial = initial_status.is_none_or(|status| status != until);

    loop {
        let agent = snapshot(target)?;
        let status = agent.status.as_str();

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

        if started_at.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                WaitTimeoutError::new(target, until, status),
            ));
        }

        sleep(Duration::from_millis(250));
    }
}

fn status_marker_changed(initial: &AgentIdentity, current: &AgentIdentity) -> bool {
    initial.uuid == current.uuid
        && initial.status == current.status
        && initial.last_status_at != current.last_status_at
}

fn wait_target_snapshot(target: &str) -> io::Result<AgentIdentity> {
    if target == "all" || target.starts_with("class:") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "wait requires a single agent name or uuid",
        ));
    }

    list_agents()?
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
    use std::sync::{Mutex, OnceLock};
    use wardian_core::control::MessageOrigin;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn spawn_and_clone_use_longer_control_timeout() {
        assert!(operation_timeout(&ControlOperation::AgentSpawn) > CONTROL_TIMEOUT);
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
    fn send_message_uses_mutation_timeout() {
        assert_eq!(
            operation_timeout(&ControlOperation::SendMessage),
            CONTROL_MUTATION_TIMEOUT
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
    fn queued_delivery_message_ids_only_returns_queued_ids_with_ids() {
        let delivery = vec![
            delivery_detail("queued", Some("msg_1")),
            delivery_detail("submit_started", Some("msg_2")),
            delivery_detail("queued", None),
        ];

        assert_eq!(queued_delivery_message_ids(&delivery), vec!["msg_1"]);
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
    fn queued_submission_prewait_applies_only_to_output_and_status_conditions() {
        assert!(condition_requires_queued_delivery_submission("output:DONE"));
        assert!(condition_requires_queued_delivery_submission("status:idle"));
        assert!(!condition_requires_queued_delivery_submission(
            "delivery:submit_sent_unverified"
        ));
        assert!(!condition_requires_queued_delivery_submission(
            "delivery:queued"
        ));
        assert!(!condition_requires_queued_delivery_submission(
            "event:custom"
        ));
    }

    #[test]
    fn wait_target_rejects_multi_target_selectors() {
        let error = wait_target_snapshot("all").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    fn agent(status: &str, last_status_at: Option<&str>) -> AgentIdentity {
        AgentIdentity {
            name: "reviewer-a1".to_string(),
            uuid: "uuid-1".to_string(),
            class: "Reviewer".to_string(),
            provider: "codex".to_string(),
            status: status.to_string(),
            pid: Some(42),
            started_at: Some("2026-05-07T12:00:00.000Z".to_string()),
            workspace: Some("D:/Development/Wardian".to_string()),
            last_status_at: last_status_at.map(str::to_string),
            status_source: wardian_core::identity::StatusSource::Live,
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
            |_| Ok(completed.clone()),
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
            |_| {
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
