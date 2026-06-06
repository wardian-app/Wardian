use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;
use tauri::Manager;
use wardian_core::control::{
    InteractionBodyRef, ProviderInputReadiness, ProviderReadyEvidence, ReplyStatus,
};
use wardian_core::models::AgentConfig;

type AgentRunFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
type LiveAgentRunFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

/// What the executor needs to run one headless agent prompt.
#[derive(Debug, Clone)]
pub struct AgentRunSpec {
    pub node: String,
    pub provider: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub session_id: String,
    pub resume_session: Option<String>,
    pub config_override: Option<AgentConfig>,
}

/// What the executor needs to route one prompt into an already-running agent.
#[derive(Debug, Clone)]
pub struct LiveAgentRunSpec {
    pub node: String,
    pub session_id: String,
    pub prompt: String,
    pub timeout: Duration,
}

/// Boundary between the executor logic and the real headless runtime. Unit tests
/// inject a fake so they never spawn a provider.
pub trait AgentRunner: Send + Sync {
    /// Run the prompt headlessly; return the agent's textual response.
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_>;
}

/// Boundary for active-agent execution. Unlike headless runs, this uses the
/// existing live PTY and completes only through the structured `wardian reply`
/// contract. Idle terminal status is not completion; it is only a signal that
/// transcript marker compatibility can be checked.
pub trait LiveAgentRunner: Send + Sync {
    fn run_live(&self, spec: LiveAgentRunSpec) -> LiveAgentRunFuture<'_>;
}

/// Real runner: drives `manager::headless::run_headless_with_options` and pulls
/// the `response` field out of the provider-normalized JSON.
pub struct HeadlessAgentRunner;

impl AgentRunner for HeadlessAgentRunner {
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_> {
        Box::pin(async move {
            let value =
                crate::manager::run_headless_with_options(crate::manager::HeadlessRunOptions {
                    cwd: &spec.cwd,
                    prompt: &spec.prompt,
                    wardian_session_id: &spec.session_id,
                    resume_session: spec.resume_session.as_deref(),
                    output_format: "json",
                    provider_name: &spec.provider,
                    config_override: spec.config_override.as_ref(),
                })
                .await?;

            let response = value
                .get("response")
                .and_then(|value| value.as_str())
                .or_else(|| value.get("text").and_then(|value| value.as_str()))
                .map(ToString::to_string)
                .unwrap_or_else(|| value.to_string());

            Ok(response)
        })
    }
}

#[derive(Clone)]
pub struct TauriLiveAgentRunner {
    app: tauri::AppHandle,
}

impl TauriLiveAgentRunner {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl LiveAgentRunner for TauriLiveAgentRunner {
    fn run_live(&self, spec: LiveAgentRunSpec) -> LiveAgentRunFuture<'_> {
        Box::pin(async move { run_live_agent_prompt(&self.app, spec).await })
    }
}

async fn run_live_agent_prompt(
    app: &tauri::AppHandle,
    spec: LiveAgentRunSpec,
) -> Result<String, String> {
    let state = app.state::<crate::state::AppState>();
    let delivery_lock = state.delivery_lock_for(&spec.session_id).await;
    let _delivery_guard = delivery_lock.lock().await;

    let (tx, watch_state, current_status, provider_name, should_mark_processing) = {
        let agents = state.agents.lock().await;
        let agent = agents
            .get(&spec.session_id)
            .ok_or_else(|| format!("Agent {} not found or is off", spec.session_id))?;
        let config = agent
            .config
            .lock()
            .map_err(|_| format!("Agent {} config lock poisoned", spec.session_id))?
            .clone();
        let tx = state
            .input_senders
            .try_read()
            .map_err(|_| "Input channel temporarily locked".to_string())?
            .get(&spec.session_id)
            .cloned()
            .ok_or_else(|| format!("Agent {} not found or is off", spec.session_id))?;
        (
            tx,
            agent.watch_state.clone(),
            agent.current_status.clone(),
            config.provider,
            crate::manager::mark_agent_prompt_started(agent),
        )
    };

    let initial_cursor = watch_state
        .lock()
        .map_err(|_| format!("Agent {} watch state lock poisoned", spec.session_id))?
        .latest_cursor();

    let task = state
        .interactions
        .create_task(
            None,
            spec.session_id.clone(),
            InteractionBodyRef::Inline {
                body: spec.prompt.clone(),
            },
        )
        .await;
    if let Err(error) = (|| {
        let mut guard = watch_state
            .lock()
            .map_err(|_| format!("Agent {} watch state lock poisoned", spec.session_id))?;
        guard.push_event(
            "request",
            serde_json::json!({
                "request_id": &task.id,
                "target_session_id": &spec.session_id,
                "status": "pending",
                "created_at": &task.created_at,
                "workflow_node": &spec.node,
            }),
        );
        Ok::<(), String>(())
    })() {
        fail_live_workflow_task(
            &state,
            &watch_state,
            &task.id,
            &spec.session_id,
            &error,
            false,
        )
        .await;
        return Err(error);
    }

    if should_mark_processing {
        crate::manager::set_agent_status(app, &spec.session_id, &current_status, "Processing...");
    }
    let provider_input = state
        .interactions
        .start_provider_input_generation(&spec.session_id, ProviderInputReadiness::Busy, None)
        .await;

    let prompt = prompt_with_structured_reply_instruction(&spec.prompt, &task.id);
    if let Err(error) = crate::commands::terminal::submit_prompt_to_agent_with_codex_echo_guard(
        &state,
        &spec.session_id,
        &provider_name,
        &tx,
        &prompt,
    )
    .await
    {
        let message = format!(
            "failed to submit workflow node {} to live agent {}: {error}",
            spec.node, spec.session_id
        );
        fail_live_workflow_task(
            &state,
            &watch_state,
            &task.id,
            &spec.session_id,
            &message,
            true,
        )
        .await;
        return Err(message);
    }

    let reply = match wait_for_live_agent_reply(
        &state,
        watch_state.clone(),
        initial_cursor,
        &task.id,
        &spec.session_id,
        spec.timeout,
    )
    .await
    {
        Ok(reply) => reply,
        Err(error) => {
            fail_live_workflow_task(
                &state,
                &watch_state,
                &task.id,
                &spec.session_id,
                &error,
                true,
            )
            .await;
            return Err(error);
        }
    };
    let response = match reply.status {
        ReplyStatus::Done => reply.body,
        ReplyStatus::Blocked | ReplyStatus::Failed => {
            release_live_agent_provider_input(&state, &spec.session_id).await;
            return Err(format!(
                "live agent {} returned {:?} for workflow node {}: {}",
                spec.session_id, reply.status, spec.node, reply.body
            ));
        }
    };
    if response.trim().is_empty() {
        release_live_agent_provider_input(&state, &spec.session_id).await;
        Err(format!(
            "live agent {} completed workflow node {} without readable output",
            spec.session_id, spec.node
        ))
    } else if let Err(error) = wait_for_live_agent_provider_ready(
        &state,
        &spec.session_id,
        provider_input.generation,
        Duration::from_secs(30),
    )
    .await
    {
        release_live_agent_provider_input(&state, &spec.session_id).await;
        Err(error)
    } else {
        Ok(response)
    }
}

fn prompt_with_structured_reply_instruction(prompt: &str, request_id: &str) -> String {
    format!(
        "{prompt}\n\nWardian workflow request id: {request_id}\nWhen this workflow node is fully complete, respond with:\nwardian reply {request_id} --status done --stdin\nUse --status blocked or --status failed if you cannot complete it. Put the final workflow output on stdin."
    )
}

async fn fail_live_workflow_task(
    app_state: &crate::state::AppState,
    watch_state: &std::sync::Arc<Mutex<crate::state::AgentWatchState>>,
    request_id: &str,
    target_session_id: &str,
    reason: &str,
    release_provider_input: bool,
) {
    if release_provider_input {
        release_live_agent_provider_input(app_state, target_session_id).await;
    }

    let Ok(reply) = app_state
        .interactions
        .fail_task_with_reply(request_id, target_session_id, reason)
        .await
    else {
        return;
    };

    if let Ok(mut guard) = watch_state.lock() {
        guard.push_event(
            "reply",
            serde_json::json!({
                "request_id": reply.request_id,
                "status": reply.status,
                "target_session_id": reply.target_session_id,
                "source_session_id": reply.source_session_id,
                "replied_at": reply.replied_at,
            }),
        );
    }
}

async fn release_live_agent_provider_input(
    app_state: &crate::state::AppState,
    target_session_id: &str,
) {
    if let Some(generation) = app_state
        .interactions
        .current_provider_input_generation(target_session_id)
        .await
    {
        app_state
            .interactions
            .record_provider_input_state(
                target_session_id,
                generation,
                ProviderInputReadiness::Unknown,
                None,
            )
            .await;
    }
}

async fn wait_for_live_agent_provider_ready(
    app_state: &crate::state::AppState,
    target_session_id: &str,
    generation: u64,
    timeout: Duration,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    loop {
        if let Some(input_state) = app_state
            .interactions
            .provider_input_state(target_session_id)
            .await
        {
            if input_state.generation >= generation
                && input_state.state == ProviderInputReadiness::Ready
                && input_state.ready_evidence == Some(ProviderReadyEvidence::ProviderEvent)
            {
                return Ok(());
            }
            if input_state.generation >= generation
                && matches!(
                    input_state.state,
                    ProviderInputReadiness::ActionRequired | ProviderInputReadiness::Unavailable
                )
            {
                return Err(format!(
                    "live agent {target_session_id} did not return to input-ready state after workflow reply"
                ));
            }
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(format!(
                "timed out waiting for live agent {target_session_id} to return to input-ready state"
            ));
        }
        tokio::time::sleep((timeout - elapsed).min(Duration::from_millis(25))).await;
    }
}

async fn wait_for_live_agent_reply(
    app_state: &crate::state::AppState,
    watch_state: std::sync::Arc<Mutex<crate::state::AgentWatchState>>,
    since: String,
    request_id: &str,
    target_session_id: &str,
    timeout: Duration,
) -> Result<wardian_core::control::StructuredReply, String> {
    let started = std::time::Instant::now();
    loop {
        if let Some(reply) = app_state.interactions.structured_reply(request_id).await {
            return Ok(reply);
        }
        let snapshot = {
            let guard = watch_state
                .lock()
                .map_err(|_| "watch state lock poisoned".to_string())?;
            guard
                .snapshot_since(Some(&since), Some(128 * 1024))
                .map_err(|error| format!("watch state error: {}", error.code()))?
        };
        if let Some(error) = live_agent_terminal_failure(&snapshot) {
            return Err(error);
        }
        if latest_terminal_status(&snapshot).as_deref() == Some("idle") {
            if let Some((status, body)) = structured_reply_marker(&snapshot, request_id) {
                return app_state
                    .interactions
                    .complete_task_with_reply(request_id, Some(target_session_id), status, &body)
                    .await
                    .map_err(|error| {
                        format!("failed to record live agent workflow reply {request_id}: {error}")
                    });
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(format!(
                "timed out waiting for live agent workflow reply {request_id}"
            ));
        }
        tokio::time::sleep((timeout - elapsed).min(Duration::from_millis(25))).await;
    }
}

fn structured_reply_marker(
    snapshot: &crate::state::agent_watch::WatchSnapshot,
    request_id: &str,
) -> Option<(ReplyStatus, String)> {
    snapshot
        .transcript
        .messages
        .iter()
        .rev()
        .filter(|message| matches!(message.role.as_str(), "assistant" | "model"))
        .find_map(|message| parse_structured_reply_marker(&message.text, request_id))
}

fn parse_structured_reply_marker(text: &str, request_id: &str) -> Option<(ReplyStatus, String)> {
    let marker = format!("wardian reply {request_id}");
    let lines = text.lines().collect::<Vec<_>>();
    let marker_line_index = lines.iter().position(|line| {
        let trimmed = line.trim();
        trimmed.starts_with(&marker)
            && trimmed
                .get(marker.len()..)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(' '))
    })?;
    let command = lines[marker_line_index].trim();
    if !command.contains("--stdin") {
        return None;
    }
    let status = if command.contains("--status blocked") {
        ReplyStatus::Blocked
    } else if command.contains("--status failed") {
        ReplyStatus::Failed
    } else if command.contains("--status done") {
        ReplyStatus::Done
    } else {
        return None;
    };
    let leading_body = lines[..marker_line_index].join("\n").trim().to_string();
    let trailing_body = lines[marker_line_index + 1..].join("\n");
    let body = if leading_body.is_empty() {
        trailing_body.trim().to_string()
    } else {
        leading_body
    };
    (!body.is_empty()).then_some((status, body))
}

#[cfg(test)]
async fn wait_for_live_agent_status_transition_for_regression(
    state: std::sync::Arc<Mutex<crate::state::AgentWatchState>>,
    since: String,
    timeout: Duration,
) -> Result<crate::state::agent_watch::WatchSnapshot, String> {
    let started = std::time::Instant::now();
    let notify = state
        .lock()
        .map_err(|_| "watch state lock poisoned".to_string())?
        .notifier();

    loop {
        let notified = notify.notified();
        let snapshot = {
            let guard = state
                .lock()
                .map_err(|_| "watch state lock poisoned".to_string())?;
            guard
                .snapshot_since(Some(&since), Some(128 * 1024))
                .map_err(|error| format!("watch state error: {}", error.code()))?
        };

        if let Some(error) = live_agent_terminal_failure(&snapshot) {
            return Err(error);
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err("timed out waiting for live agent workflow node to complete".to_string());
        }
        if tokio::time::timeout(timeout - elapsed, notified)
            .await
            .is_err()
        {
            return Err("timed out waiting for live agent workflow node to complete".to_string());
        }
    }
}

fn live_agent_terminal_failure(
    snapshot: &crate::state::agent_watch::WatchSnapshot,
) -> Option<String> {
    latest_terminal_status(snapshot).and_then(|status| match status.as_str() {
        "action_required" | "off" | "error" => Some(format!(
            "live agent reached {status} before completing workflow node"
        )),
        _ => None,
    })
}

fn latest_terminal_status(snapshot: &crate::state::agent_watch::WatchSnapshot) -> Option<String> {
    snapshot.events.iter().rev().find_map(|event| {
        (event.kind == "status")
            .then(|| event.payload.get("status").and_then(|value| value.as_str()))
            .flatten()
            .map(wardian_core::identity::normalize_status)
    })
}

/// Test double: scripted responses keyed by node id; records call order.
#[derive(Default)]
pub struct FakeAgentRunner {
    responses: HashMap<String, String>,
    calls: Mutex<Vec<String>>,
}

impl FakeAgentRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_response(mut self, node: &str, response: &str) -> Self {
        self.responses.insert(node.into(), response.into());
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("fake runner calls lock").clone()
    }
}

impl AgentRunner for FakeAgentRunner {
    fn run(&self, spec: AgentRunSpec) -> AgentRunFuture<'_> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("fake runner calls lock")
                .push(spec.node.clone());

            Ok(self
                .responses
                .get(&spec.node)
                .cloned()
                .unwrap_or_else(|| "{}".into()))
        })
    }
}

/// Test double for active-agent execution.
#[derive(Default)]
pub struct FakeLiveAgentRunner {
    responses: HashMap<String, String>,
    calls: Mutex<Vec<String>>,
}

impl FakeLiveAgentRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_response(mut self, session_id: &str, response: &str) -> Self {
        self.responses.insert(session_id.into(), response.into());
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("fake live runner calls lock")
            .clone()
    }
}

impl LiveAgentRunner for FakeLiveAgentRunner {
    fn run_live(&self, spec: LiveAgentRunSpec) -> LiveAgentRunFuture<'_> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("fake live runner calls lock")
                .push(spec.session_id.clone());

            Ok(self
                .responses
                .get(&spec.session_id)
                .cloned()
                .unwrap_or_else(|| "{}".into()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn fake_runner_returns_scripted_response_and_records_calls() {
        let runner = FakeAgentRunner::new().with_response("plan", "```json\n{\"ok\":true}\n```");
        let spec = AgentRunSpec {
            node: "plan".into(),
            provider: "mock".into(),
            cwd: std::path::PathBuf::from("."),
            prompt: "do".into(),
            session_id: String::new(),
            resume_session: None,
            config_override: None,
        };
        let out = runner.run(spec).await.unwrap();
        assert!(out.contains("ok"));
        assert_eq!(runner.calls(), vec!["plan".to_string()]);
    }

    #[tokio::test]
    async fn live_agent_wait_does_not_complete_on_idle_status_alone() {
        let state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            4096,
        )));
        let since = state.lock().expect("watch state lock").latest_cursor();
        {
            let mut guard = state.lock().expect("watch state lock");
            guard.push_output(b"stale terminal repaint text");
            guard.push_event("status", serde_json::json!({ "status": "idle" }));
        }

        let result = wait_for_live_agent_status_transition_for_regression(
            state,
            since,
            Duration::from_millis(10),
        )
        .await;

        assert_eq!(
            result.expect_err("idle status alone must not complete a live workflow node"),
            "timed out waiting for live agent workflow node to complete"
        );
    }

    #[tokio::test]
    async fn live_agent_reply_wait_completes_only_after_structured_reply() {
        let app_state = crate::state::AppState::new();
        let watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            4096,
        )));
        let since = watch_state
            .lock()
            .expect("watch state lock")
            .latest_cursor();
        {
            let mut guard = watch_state.lock().expect("watch state lock");
            guard.push_output(b"stale terminal repaint text");
            guard.push_event("status", serde_json::json!({ "status": "idle" }));
        }
        let task = app_state
            .interactions
            .create_task_with_id(
                "wf_test_reply".to_string(),
                None,
                "agent-1".to_string(),
                InteractionBodyRef::Inline {
                    body: "write the file".to_string(),
                },
            )
            .await;
        let task_id = task.id.clone();

        let wait = wait_for_live_agent_reply(
            &app_state,
            watch_state,
            since,
            &task_id,
            "agent-1",
            Duration::from_secs(1),
        );
        let complete = async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            app_state
                .interactions
                .complete_task_with_reply(
                    &task_id,
                    Some("agent-1"),
                    ReplyStatus::Done,
                    "{\"ok\":true}",
                )
                .await
                .expect("complete workflow task")
        };

        let (reply, _) = tokio::join!(wait, complete);

        let reply = reply.expect("structured reply should complete workflow node");
        assert_eq!(reply.status, ReplyStatus::Done);
        assert_eq!(reply.body, "{\"ok\":true}");
    }

    #[tokio::test]
    async fn live_agent_reply_wait_accepts_explicit_transcript_marker_after_idle() {
        let app_state = crate::state::AppState::new();
        let watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            4096,
        )));
        let since = watch_state
            .lock()
            .expect("watch state lock")
            .latest_cursor();
        let task = app_state
            .interactions
            .create_task_with_id(
                "wf_test_marker".to_string(),
                None,
                "agent-1".to_string(),
                InteractionBodyRef::Inline {
                    body: "write the file".to_string(),
                },
            )
            .await;
        let task_id = task.id.clone();
        {
            let mut guard = watch_state.lock().expect("watch state lock");
            guard.push_transcript(wardian_core::control::WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: format!(
                    "Final workflow output\n\nwardian reply {task_id} --status done --stdin"
                ),
                provider: "gemini".to_string(),
                turn_id: None,
                source: Some("gemini_log".to_string()),
            });
            guard.push_event("status", serde_json::json!({ "status": "idle" }));
        }

        let reply = wait_for_live_agent_reply(
            &app_state,
            watch_state,
            since,
            &task_id,
            "agent-1",
            Duration::from_secs(1),
        )
        .await
        .expect("marker should complete workflow task");

        assert_eq!(reply.status, ReplyStatus::Done);
        assert_eq!(reply.body, "Final workflow output");
        assert!(app_state
            .interactions
            .structured_reply(&task_id)
            .await
            .is_some());
    }

    #[tokio::test]
    async fn live_agent_failure_cleanup_closes_task_and_releases_input_generation() {
        let app_state = crate::state::AppState::new();
        let watch_state = Arc::new(Mutex::new(crate::state::AgentWatchState::new(
            "agent-1".to_string(),
            16,
            4096,
        )));
        let task = app_state
            .interactions
            .create_task_with_id(
                "wf_test_cleanup".to_string(),
                None,
                "agent-1".to_string(),
                InteractionBodyRef::Inline {
                    body: "write the file".to_string(),
                },
            )
            .await;
        app_state
            .interactions
            .start_provider_input_generation("agent-1", ProviderInputReadiness::Busy, None)
            .await;

        fail_live_workflow_task(
            &app_state,
            &watch_state,
            &task.id,
            "agent-1",
            "timed out",
            true,
        )
        .await;

        assert_eq!(
            app_state
                .interactions
                .interaction(&task.id)
                .await
                .unwrap()
                .status,
            wardian_core::control::InteractionStatus::Failed
        );
        assert_eq!(
            app_state
                .interactions
                .structured_reply(&task.id)
                .await
                .unwrap()
                .status,
            ReplyStatus::Failed
        );
        assert_eq!(
            app_state
                .interactions
                .provider_input_state("agent-1")
                .await
                .unwrap()
                .state,
            ProviderInputReadiness::Unknown
        );
        let snapshot = watch_state
            .lock()
            .expect("watch state lock")
            .snapshot_since(None, None)
            .expect("watch snapshot");
        assert!(snapshot.events.iter().any(|event| {
            event.kind == "reply"
                && event
                    .payload
                    .get("request_id")
                    .and_then(|value| value.as_str())
                    == Some("wf_test_cleanup")
                && event.payload.get("status").and_then(|value| value.as_str()) == Some("failed")
        }));
    }

    #[tokio::test]
    async fn live_agent_success_waits_for_provider_ready_evidence() {
        let app_state = crate::state::AppState::new();
        let input = app_state
            .interactions
            .start_provider_input_generation("agent-1", ProviderInputReadiness::Busy, None)
            .await;

        let wait = wait_for_live_agent_provider_ready(
            &app_state,
            "agent-1",
            input.generation,
            Duration::from_secs(1),
        );
        let ready = async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            app_state
                .interactions
                .record_provider_input_state(
                    "agent-1",
                    input.generation,
                    ProviderInputReadiness::Ready,
                    Some(ProviderReadyEvidence::ProviderEvent),
                )
                .await;
        };

        let (result, _) = tokio::join!(wait, ready);

        result.expect("provider event readiness should release live workflow delivery");
    }

    #[test]
    fn structured_reply_marker_requires_body_and_status() {
        assert!(parse_structured_reply_marker(
            "wardian reply wf_test --status done --stdin",
            "wf_test"
        )
        .is_none());
        assert!(
            parse_structured_reply_marker("body\nwardian reply wf_test --stdin", "wf_test")
                .is_none()
        );
    }

    #[test]
    fn structured_reply_marker_accepts_body_after_command() {
        let (status, body) = parse_structured_reply_marker(
            "wardian reply wf_test --status done --stdin\nSummary written.",
            "wf_test",
        )
        .expect("marker-first reply should parse");

        assert_eq!(status, ReplyStatus::Done);
        assert_eq!(body, "Summary written.");
    }

    #[test]
    fn structured_reply_marker_ignores_embedded_command_text() {
        assert!(parse_structured_reply_marker(
            "Please run wardian reply wf_test --status done --stdin after the work.\nSummary.",
            "wf_test",
        )
        .is_none());
    }
}
